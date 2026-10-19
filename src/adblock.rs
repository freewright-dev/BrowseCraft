//! Ported from uBlock Origin's core: a static network-filter engine and
//! cosmetic (element-hiding) filters, consuming EasyList/uBO filter syntax.
//!
//! The parts adapted from `uBlock-master/src/js` are: `static-net-filtering.js`
//! (rule matching, `@@` exception priority, `$` options), the relevant slice of
//! `static-filtering-parser.js` (ABP/uBO syntax) and `cosmetic-filtering.js`
//! (`##` element hiding). The extension machinery (`platform/`, `vAPI`,
//! `webext.js`) maps to three sinks instead, all driven from this module:
//!
//! 1. Rust-side: main-frame/popup navigations and engine-page subresources are
//!    checked before anything loads (`matches_network`).
//! 2. WebView2 pages, natively: `browser.rs` answers `WebResourceRequested`
//!    with `blocked_request`, so every request of a page is judged before it
//!    is sent (this also covers tags the HTML parser inserts).
//! 3. WebView2 pages, in the document: `page_script_with()` emits the
//!    equivalent of uBO's `contentscript.js`: element hiding, and, where there
//!    is no native interception, fetch/XHR/beacon/img/script/iframe blocking.
//! 4. Engine pages: `engine_cosmetic_css()` returns a stylesheet applying
//!    `display: none` through the built-in selector engine.
//!
//! Limitations vs uBO: no regex filters (`/re/`), no `$redirect`/`$csp`/
//! `$replace`/scriptlet/procedural filters (rules using them are dropped, never
//! over-blocking), and third-party is computed with a hostname-suffix check
//! instead of the PSL.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// ---- Request types (bitmask, mirrors uBO's type bits) ----

pub const TYPE_MAIN_FRAME: u32 = 1 << 0;
pub const TYPE_SUB_FRAME: u32 = 1 << 1;
pub const TYPE_SCRIPT: u32 = 1 << 2;
pub const TYPE_IMAGE: u32 = 1 << 3;
pub const TYPE_STYLESHEET: u32 = 1 << 4;
pub const TYPE_FONT: u32 = 1 << 5;
pub const TYPE_XHR: u32 = 1 << 6;
pub const TYPE_WEBSOCKET: u32 = 1 << 7;
pub const TYPE_MEDIA: u32 = 1 << 8;
pub const TYPE_PING: u32 = 1 << 9;
pub const TYPE_OTHER: u32 = 1 << 10;
pub const TYPE_POPUP: u32 = 1 << 11;

const TYPE_DOC_PAGE: u32 = TYPE_MAIN_FRAME | TYPE_POPUP | TYPE_OTHER;
const TYPE_ALL: u32 = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    Neutral,
    Block,
    Allow,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Party {
    #[default]
    Any,
    First,
    Third,
}

// ---- Compiled network rule ----

#[derive(Default)]
struct Options {
    types: Option<u32>,
    party: Party,
    domains: Vec<DomainSel>,
    important: bool,
}

#[derive(Clone)]
struct DomainSel {
    host: String,
    neg: bool,
}

#[derive(Clone, Debug)]
enum Tok {
    Lit(String),
    Sep,
    Any,
}

struct Pattern {
    /// `||host...` — matched against the URL hostname (host or subdomains).
    host: String,
    toks: Vec<Tok>,
    /// `|` left anchor (start-of-URL). Non-host rules only.
    left_anchor: bool,
    /// trailing `|` (end-of-URL).
    right_anchor: bool,
}

struct NetRule {
    pattern: Pattern,
    options: Options,
    allow: bool,
}

// ---- Compiled cosmetic rule ----

struct Cosmetic {
    /// Empty = applies to every document. Otherwise the comma-separated host
    /// list of the filter: it applies when any positive host matches (or there
    /// are only negated ones) and no negated host matches.
    hosts: Vec<DomainSel>,
    selector: String,
    allow: bool,
}

pub struct AdBlocker {
    /// Rules keyed by the hostname they are anchored on, for fast lookup.
    by_host: HashMap<String, Vec<usize>>,
    /// Every rule that is not `||host`-anchored (used to emit the JS tables).
    plain: Vec<usize>,
    /// The same rules as `plain`, indexed by one literal URL token (the way
    /// uBO's `static-net-filtering.js` buckets filters), so that a request
    /// only tries the few rules that can possibly match it.
    by_token: HashMap<String, Vec<usize>>,
    /// Non-host rules for which no safe token exists: tried on every request.
    loose: Vec<usize>,
    rules: Vec<NetRule>,
    cosmetic: Vec<Cosmetic>,
}

// ---- Public API ----

impl AdBlocker {
    pub fn empty() -> Self {
        AdBlocker {
            by_host: HashMap::new(),
            plain: Vec::new(),
            by_token: HashMap::new(),
            loose: Vec::new(),
            rules: Vec::new(),
            cosmetic: Vec::new(),
        }
    }

    pub fn add_filter_list(&mut self, _name: &str, content: &str) {
        // `lines()` would miss classic Mac (CR-only) line endings.
        for line in content.split(['\n', '\r']) {
            self.add_filter_line(line.trim());
        }
    }

    /// The network verdict for one request. `url_host` and `doc_host` are the
    /// hostnames (no scheme/port, lowercase). `type_bits` is a `TYPE_*` mask.
    pub fn matches_network(
        &self,
        url: &str,
        url_host: &str,
        type_bits: u32,
        doc_host: &str,
    ) -> Decision {
        // Match against the parsed URL: its serialization is ASCII (punycode
        // hosts, percent-encoded paths) and says exactly where the host ends,
        // even with `user@`, a port or an IDN typed as Unicode.
        let parsed = url::Url::parse(url).ok();
        let (hay, host_end) = match &parsed {
            Some(u) => (
                u.as_str().to_ascii_lowercase(),
                Some(u[..url::Position::AfterHost].len()),
            ),
            None => (url.to_lowercase(), None),
        };
        let party = if doc_host.is_empty() {
            Party::Third
        } else {
            party_of(url_host, doc_host)
        };

        let mut block = false;
        let mut allow = false;
        let mut block_important = false;

        let mut consider = |rule: &NetRule| {
            if rule_hits(rule, &hay, host_end, url_host, party, type_bits, doc_host) {
                if rule.options.important && !rule.allow {
                    block_important = true;
                }
                if rule.allow {
                    allow = true;
                } else {
                    block = true;
                }
            }
        };

        for &i in &self.loose {
            consider(&self.rules[i]);
        }
        let mut seen: Vec<&str> = Vec::new();
        for tok in hay.split(|c: char| !c.is_ascii_alphanumeric()) {
            if tok.len() < MIN_TOKEN || seen.contains(&tok) {
                continue;
            }
            seen.push(tok);
            if let Some(indices) = self.by_token.get(tok) {
                for &i in indices {
                    consider(&self.rules[i]);
                }
            }
        }
        if !url_host.is_empty() {
            for key in host_suffixes(url_host) {
                if let Some(indices) = self.by_host.get(key.as_str()) {
                    for &i in indices {
                        consider(&self.rules[i]);
                    }
                }
            }
        }

        if block_important || (block && !allow) {
            Decision::Block
        } else if block && allow {
            Decision::Allow
        } else {
            Decision::Neutral
        }
    }

    /// A one-line summary of what was loaded (for diagnostics).
    #[allow(dead_code)]
    pub fn stats(&self) -> String {
        format!(
            "{} network rules ({} host-anchored, {} token-indexed, {} untokenised), {} cosmetic rules",
            self.rules.len(),
            self.by_host.values().map(Vec::len).sum::<usize>(),
            self.by_token.values().map(Vec::len).sum::<usize>(),
            self.loose.len(),
            self.cosmetic.len()
        )
    }

    /// Hide selectors applying to `doc_host`, exceptions (`#@#`) subtracted.
    pub fn cosmetic_selectors(&self, doc_host: &str) -> Vec<String> {
        let mut hide: Vec<String> = Vec::new();
        let mut allow: HashSet<&str> = HashSet::new();
        for c in &self.cosmetic {
            if cosmetic_applies(&c.hosts, doc_host) {
                if c.allow {
                    allow.insert(c.selector.as_str());
                } else {
                    hide.push(c.selector.clone());
                }
            }
        }
        let mut seen: HashSet<String> = HashSet::new();
        hide.into_iter()
            .filter(|s| !allow.contains(s.as_str()) && seen.insert(s.clone()))
            .collect()
    }

    /// A `<style>` body for engine-rendered pages.
    pub fn engine_cosmetic_css(&self, doc_host: &str) -> String {
        let sels = self.cosmetic_selectors(doc_host);
        if sels.is_empty() {
            return String::new();
        }
        sels.iter()
            .map(|s| format!("{s} {{ display: none !important }}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The WebView2 content script with network blocking included (what a
    /// platform without native request interception needs).
    #[allow(dead_code)]
    pub fn page_script(&self) -> String {
        self.page_script_with(true)
    }

    /// The WebView2 content script. It always injects the element-hiding
    /// stylesheet (top document only). With `network` it also carries the
    /// compiled network rules and cancels matching fetch/XHR/beacon/img/script/
    /// iframe loads in every frame; pass `false` when the host intercepts
    /// requests natively (WebView2's `WebResourceRequested`), which sees
    /// parser-inserted tags too and keeps the script small.
    pub fn page_script_with(&self, network: bool) -> String {
        let mut url_rules: Vec<String> = Vec::new();
        let mut host_map: Vec<String> = Vec::new();
        if network {
            for &i in &self.plain {
                let r = &self.rules[i];
                let re = token_regex(&r.pattern.toks, r.pattern.left_anchor, r.pattern.right_anchor);
                url_rules.push(js_entry(&re, r));
            }
            url_rules.sort();
            for (host, indices) in &self.by_host {
                let entries: Vec<String> = indices
                    .iter()
                    .map(|&i| {
                        let r = &self.rules[i];
                        js_entry(&token_regex(&r.pattern.toks, false, r.pattern.right_anchor), r)
                    })
                    .collect();
                if !entries.is_empty() {
                    host_map.push(format!("{}:[{}]", quote(host), entries.join(",")));
                }
            }
            host_map.sort();
        }

        // Exceptions without a host list remove their selector everywhere.
        let generic_allow: HashSet<&str> = self
            .cosmetic
            .iter()
            .filter(|c| c.allow && c.hosts.is_empty())
            .map(|c| c.selector.as_str())
            .collect();
        let mut generic: Vec<String> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut scoped: Vec<String> = Vec::new();
        for c in &self.cosmetic {
            if generic_allow.contains(c.selector.as_str()) {
                continue;
            }
            if c.hosts.is_empty() {
                if !c.allow && seen.insert(c.selector.as_str()) {
                    generic.push(quote(&c.selector));
                }
            } else {
                scoped.push(format!(
                    "[{},{},{}]",
                    c.allow as u8,
                    js_hosts(&c.hosts),
                    quote(&c.selector)
                ));
            }
        }
        scoped.sort();
        scoped.dedup();

        // Big tables go through JSON.parse of one string literal: far faster
        // for V8 to load than an equally large object/array literal.
        let json = |text: String| format!("JSON.parse({})", quote(&text));
        JS_HOOK
            .replace("__SCRIPTLETS__", YT_SCRIPTLET)
            .replace("__NET__", if network { "true" } else { "false" })
            .replace("__HOST_RULES__", &json(format!("{{{}}}", host_map.join(","))))
            .replace("__URL_RULES__", &json(format!("[{}]", url_rules.join(","))))
            .replace("__GEN_CSS__", &json(format!("[{}]", generic.join(","))))
            .replace("__SCOPED_CSS__", &json(format!("[{}]", scoped.join(","))))
    }

    fn add_filter_line(&mut self, line: &str) {
        if line.is_empty()
            || line.starts_with('!')
            || line.starts_with('[')
            || line.starts_with("#!#")
        {
            return;
        }
        // Procedural filters, scriptlets and style injections need uBO's
        // content-script machinery: dropped, never approximated.
        if line.contains("#?#")
            || line.contains("#$#")
            || line.contains("#@$#")
            || line.contains("#$?#")
            || line.contains("##+js")
            || line.contains("#@#+js")
            || line.contains("#%#")
        {
            return;
        }
        if line.contains("##") || line.contains("#@#") {
            if let Some(c) = parse_cosmetic(line) {
                self.cosmetic.push(c);
            }
            return;
        }
        if let Some(rule) = parse_network(line) {
            let index = self.rules.len();
            self.rules.push(rule);
            if self.rules[index].pattern.host.is_empty() {
                self.plain.push(index);
                let p = &self.rules[index].pattern;
                match best_token(&p.toks, p.left_anchor, p.right_anchor) {
                    Some(tok) => self.by_token.entry(tok).or_default().push(index),
                    None => self.loose.push(index),
                }
            } else {
                self.by_host
                    .entry(self.rules[index].pattern.host.clone())
                    .or_default()
                    .push(index);
            }
        }
    }
}

/// The shared blocker built once from the bundled starter list plus every
/// `filters/*.txt` file found next to the executable or in the working
/// directory.
pub fn instance() -> &'static AdBlocker {
    static ADB: OnceLock<AdBlocker> = OnceLock::new();
    ADB.get_or_init(load_default)
}

pub fn load_default() -> AdBlocker {
    let mut ab = AdBlocker::empty();
    ab.add_filter_list("builtin:starter", START_LIST);
    let mut dirs = Vec::new();
    if let Ok(dir) = std::env::current_dir() {
        dirs.push(dir);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        dirs.push(dir.to_path_buf());
    }
    // The working directory is usually the executable's folder too: read each
    // `filters` folder once, or every rule would be loaded (and emitted) twice.
    let mut seen: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let filters = dir.join("filters");
        let key = std::fs::canonicalize(&filters).unwrap_or(filters.clone());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        for text in load_directory(&filters).into_iter().flatten() {
            ab.add_filter_list("filters", &text);
        }
    }
    ab
}

fn load_directory(path: &Path) -> Vec<Result<String, String>> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(path) else {
        return out;
    };
    let mut files: Vec<_> = rd.filter_map(Result::ok).collect();
    files.sort_by_key(|e| e.file_name().to_string_lossy().into_owned());
    for e in files {
        if e.path()
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .eq_ignore_ascii_case("txt")
        {
            out.push(std::fs::read_to_string(e.path()).map_err(|e| e.to_string()));
        }
    }
    out
}

/// Convenience for browser/popup decisions: generic filters apply too.
pub fn blocked_page(url: &str, doc_host: &str) -> bool {
    let Some(host) = host_of(url) else {
        return false;
    };
    instance().matches_network(url, &host, TYPE_DOC_PAGE, doc_host) == Decision::Block
}

/// Convenience for engine-page subresources.
pub fn blocked_subresource(url: &str, type_bits: u32, doc_host: &str) -> bool {
    let Some(host) = host_of(url) else {
        return false;
    };
    instance().matches_network(url, &host, type_bits, doc_host) == Decision::Block
}

/// Sub-resource verdict for a request made by the document at `doc_url`
/// (used by the WebView2 request interceptor).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn blocked_request(url: &str, type_bits: u32, doc_url: &str) -> bool {
    let Some(host) = host_of(url) else {
        return false;
    };
    let doc_host = host_of(doc_url).unwrap_or_default();
    instance().matches_network(url, &host, type_bits, &doc_host) == Decision::Block
}

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
}

// ---- Matching helpers ----

/// Shortest URL token worth indexing; shorter ones make huge buckets.
const MIN_TOKEN: usize = 3;

/// Tokens too common in URLs to narrow anything down.
const STOP_TOKENS: &[&str] = &["http", "https", "www", "com", "net", "org", "html", "php"];

/// The literal alphanumeric run of a pattern to index a rule under, or `None`
/// when no run is *safe*. A run is safe only when both neighbours in the
/// pattern are non-alphanumeric characters, a `^` separator or an anchor: then
/// it can only match a whole alphanumeric token of the URL, so looking URL
/// tokens up in the index never misses a rule.
fn best_token(toks: &[Tok], left_anchor: bool, right_anchor: bool) -> Option<String> {
    let mut best: Option<&str> = None;
    for (ti, t) in toks.iter().enumerate() {
        let Tok::Lit(s) = t else { continue };
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if !b[i].is_ascii_alphanumeric() {
                i += 1;
                continue;
            }
            let start = i;
            while i < b.len() && b[i].is_ascii_alphanumeric() {
                i += 1;
            }
            let end = i;
            let left_ok = start > 0
                || (ti > 0 && matches!(toks[ti - 1], Tok::Sep))
                || (ti == 0 && left_anchor);
            let right_ok = end < b.len()
                || (ti + 1 < toks.len() && matches!(toks[ti + 1], Tok::Sep))
                || (ti + 1 == toks.len() && right_anchor);
            let tok = &s[start..end];
            if left_ok
                && right_ok
                && tok.len() >= MIN_TOKEN
                && !STOP_TOKENS.contains(&tok)
                && best.is_none_or(|cur| tok.len() > cur.len())
            {
                best = Some(tok);
            }
        }
    }
    best.map(str::to_string)
}

/// Whether a cosmetic filter with this host list applies to `doc_host`.
fn cosmetic_applies(hosts: &[DomainSel], doc_host: &str) -> bool {
    let mut has_pos = false;
    let mut pos = false;
    for d in hosts {
        let m = suffix_match(doc_host, &d.host);
        if d.neg {
            if m {
                return false;
            }
        } else {
            has_pos = true;
            pos |= m;
        }
    }
    !has_pos || pos
}

fn js_hosts(hosts: &[DomainSel]) -> String {
    let items: Vec<String> = hosts
        .iter()
        .map(|d| format!("[{},{}]", quote(&d.host), d.neg as u8))
        .collect();
    format!("[{}]", items.join(","))
}

/// One compiled network rule for the JS tables:
/// `[regex, typeMask, flags(1 allow|2 important), party(0 any|1 first|2 third),
/// domains|0]`.
fn js_entry(re: &str, r: &NetRule) -> String {
    let party = match r.options.party {
        Party::Any => 0,
        Party::First => 1,
        Party::Third => 2,
    };
    let domains = if r.options.domains.is_empty() {
        "0".to_string()
    } else {
        js_hosts(&r.options.domains)
    };
    format!(
        "[{},{},{},{},{}]",
        quote(re),
        r.options.types.unwrap_or(TYPE_ALL),
        bit(r.allow, r.options.important),
        party,
        domains
    )
}

fn is_sep(c: char) -> bool {
    !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '%' | '~'))
}

fn party_of(url_host: &str, doc_host: &str) -> Party {
    if url_host == doc_host
        || url_host.ends_with(&format!(".{doc_host}"))
        || doc_host.ends_with(&format!(".{url_host}"))
    {
        Party::First
    } else {
        Party::Third
    }
}

fn suffix_match(host: &str, suffix: &str) -> bool {
    host == suffix || host.ends_with(&format!(".{}", suffix))
}

fn domain_sel_matches(d: &DomainSel, host: &str) -> bool {
    let ok = suffix_match(host, &d.host);
    if d.neg { !ok } else { ok }
}

fn host_suffixes(host: &str) -> impl Iterator<Item = String> + '_ {
    host.split('.')
        .enumerate()
        .map(move |(i, _)| host.split('.').skip(i).collect::<Vec<_>>().join("."))
}

fn rule_hits(
    rule: &NetRule,
    hay: &str,
    host_end: Option<usize>,
    url_host: &str,
    party: Party,
    type_bits: u32,
    doc_host: &str,
) -> bool {
    let o = &rule.options;
    if let Some(types) = o.types
        && types & type_bits == 0
    {
        return false;
    }
    match o.party {
        Party::Third if party != Party::Third => return false,
        Party::First if party != Party::First => return false,
        _ => {}
    }
    if !o.domains.is_empty() {
        let any_neg = o
            .domains
            .iter()
            .any(|d| d.neg && suffix_match(doc_host, &d.host));
        let has_pos = o.domains.iter().any(|d| !d.neg);
        let any_pos = o
            .domains
            .iter()
            .any(|d| !d.neg && suffix_match(doc_host, &d.host));
        if any_neg && !any_pos {
            return false;
        }
        if has_pos && !any_pos {
            return false;
        }
    }
    match_pattern(rule, hay, host_end, url_host)
}

/// `host_end` is the byte offset in `hay` just past the URL's host.
fn match_pattern(rule: &NetRule, hay: &str, host_end: Option<usize>, url_host: &str) -> bool {
    let p = &rule.pattern;
    if !p.host.is_empty() {
        if url_host != p.host && !url_host.ends_with(&format!(".{}", p.host)) {
            return false;
        }
        let Some(at) = host_end else {
            return false;
        };
        return match_tokens(hay, &p.toks, at, false, p.right_anchor);
    }
    match_tokens(hay, &p.toks, 0, !p.left_anchor, p.right_anchor)
}

/// Whether `toks` match `hay` from byte `at` (anywhere from `at` on when
/// `scan`), up to the end of `hay` when `right`.
///
/// The tokens between `*`s form segments of fixed length (a `^` may also
/// match the end of the URL, but only there). Placing each segment at its
/// leftmost match is therefore always safe, since a later placement never
/// leaves more room for what follows; only a right-anchored final segment
/// has to end exactly at the end. That makes matching linear in the URL
/// length instead of trying every placement of every segment, which with
/// several `*`s was O(n^k).
fn match_tokens(hay: &str, toks: &[Tok], at: usize, scan: bool, right: bool) -> bool {
    let end = hay.len();
    if toks.is_empty() {
        return !right || at >= end;
    }
    let trailing_any = matches!(toks.last(), Some(Tok::Any));
    let segments: Vec<&[Tok]> = toks.split(|t| matches!(t, Tok::Any)).collect();
    let last = segments.len() - 1;
    let mut cur = at;
    for (i, seg) in segments.iter().enumerate() {
        if seg.is_empty() {
            continue; // a leading or trailing `*`
        }
        // After a `*` (or anywhere, for an unanchored start) the segment may
        // start at any later position.
        let floating = i > 0 || scan;
        let must_end = i == last && right && !trailing_any;
        let found = match (floating, must_end) {
            (false, _) => match_segment_at(hay, seg, cur).filter(|&e| !must_end || e == end),
            (true, true) => {
                // The segment is at least `len - 1` bytes long.
                let from = cur.max(end.saturating_sub(segment_len(seg)));
                (from..=end).find_map(|p| match_segment_at(hay, seg, p).filter(|&e| e == end))
            }
            (true, false) => find_segment(hay, seg, cur),
        };
        match found {
            Some(e) => cur = e,
            None => return false,
        }
    }
    !right || trailing_any || cur >= end
}

/// The length of `seg` when each `^` matches one separator character.
fn segment_len(seg: &[Tok]) -> usize {
    seg.iter()
        .map(|t| match t {
            Tok::Lit(s) => s.len(),
            _ => 1,
        })
        .sum()
}

/// Where `seg` ends if it matches `hay` starting exactly at `p`.
fn match_segment_at(hay: &str, seg: &[Tok], mut p: usize) -> Option<usize> {
    for tok in seg {
        match tok {
            Tok::Lit(s) => {
                if hay.get(p..p + s.len()) != Some(s.as_str()) {
                    return None;
                }
                p += s.len();
            }
            // `^` matches a separator, or the end of the URL.
            Tok::Sep if p == hay.len() => {}
            Tok::Sep => {
                if !hay.as_bytes().get(p).is_some_and(|&b| is_sep(b as char)) {
                    return None;
                }
                p += 1;
            }
            Tok::Any => return None, // segments never contain `*`
        }
    }
    Some(p)
}

/// Where the leftmost match of `seg` at or after `from` ends.
fn find_segment(hay: &str, seg: &[Tok], from: usize) -> Option<usize> {
    let mut p = from;
    while p <= hay.len() {
        // Jump straight to occurrences of a leading literal.
        if let Some(Tok::Lit(s)) = seg.first()
            && !s.is_empty()
        {
            p += hay.get(p..)?.find(s.as_str())?;
        }
        if let Some(e) = match_segment_at(hay, seg, p) {
            return Some(e);
        }
        p += 1;
    }
    None
}

// ---- Parsing ----

fn parse_network(line: &str) -> Option<NetRule> {
    let mut pattern = line;
    let mut options_txt = "";
    if let Some(i) = line.find('$') {
        pattern = &line[..i];
        options_txt = &line[i + 1..];
    }
    let allow = pattern.starts_with("@@");
    if allow {
        pattern = &pattern[2..];
    }
    if pattern.is_empty() {
        return None;
    }
    let mut left_anchor = false;
    let mut right_anchor = false;
    let mut host = String::new();
    let toks;
    if let Some(rest) = pattern.strip_prefix("||") {
        let (h, rest) = split_host_rest(rest);
        if h.is_empty() {
            return None;
        }
        host = h.to_lowercase();
        toks = tokenize(rest);
    } else {
        if let Some(rest) = pattern.strip_prefix('|') {
            left_anchor = true;
            pattern = rest;
        }
        if let Some(rest) = pattern.strip_suffix('|') {
            right_anchor = true;
            pattern = rest;
        }
        toks = tokenize(pattern);
        if toks.is_empty() {
            return None;
        }
    }
    let options = parse_options(options_txt)?;
    Some(NetRule {
        pattern: Pattern {
            host,
            toks,
            left_anchor,
            right_anchor,
        },
        options,
        allow,
    })
}

/// Splits `||host...` into the hostname and the path/remainder. The remainder
/// keeps its leading `*`, `^` or `/`.
fn split_host_rest(s: &str) -> (String, &str) {
    for (i, c) in s.char_indices() {
        if matches!(c, '/' | '*' | '^' | '|') {
            return (s[..i].to_string(), &s[i..]);
        }
    }
    (s.to_string(), "")
}

fn tokenize(s: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut lit = String::new();
    let flush = |lit: &mut String, toks: &mut Vec<Tok>| {
        if !lit.is_empty() {
            toks.push(Tok::Lit(lit.clone()));
            lit.clear();
        }
    };
    for c in s.chars() {
        match c {
            '*' => {
                flush(&mut lit, &mut toks);
                if !matches!(toks.last(), Some(Tok::Any)) {
                    toks.push(Tok::Any);
                }
            }
            '^' => {
                flush(&mut lit, &mut toks);
                toks.push(Tok::Sep);
            }
            _ => lit.push(c.to_ascii_lowercase()),
        }
    }
    flush(&mut lit, &mut toks);
    toks
}

fn parse_options(txt: &str) -> Option<Options> {
    if txt.is_empty() {
        return Some(Options::default());
    }
    let mut o = Options::default();
    let mut saw_unsupported = false;
    for piece in txt.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        if let Some((name, value)) = piece.split_once('=') {
            match name {
                "domain" => {
                    for d in value.split('|') {
                        let d = d.trim();
                        if d.is_empty() {
                            continue;
                        }
                        let (neg, host) = if let Some(rest) = d.strip_prefix('~') {
                            (true, rest)
                        } else {
                            (false, d)
                        };
                        let host = host.strip_prefix("*.").unwrap_or(host);
                        if !host.is_empty() {
                            o.domains.push(DomainSel {
                                host: host.to_lowercase(),
                                neg,
                            });
                        }
                    }
                }
                // These change the response or request themselves; dropping the
                // rule is the safe choice (uBO would rewrite the response).
                "redirect" | "redirect-rule" | "csp" | "replace" | "removeparam" | "denyallow"
                | "method" | "header" | "from" | "to" | "urlskip" | "responseheader" => {
                    saw_unsupported = true
                }
                _ => saw_unsupported = true,
            }
        } else {
            match piece {
                "third-party" => o.party = Party::Third,
                "first-party" | "1p" => o.party = Party::First,
                "important" => o.important = true,
                "match-case" => {} // treated case-insensitively, like normal uBO non-regex filters
                "script" => o.types = Some(o.types.unwrap_or(0) | TYPE_SCRIPT),
                "image" => o.types = Some(o.types.unwrap_or(0) | TYPE_IMAGE),
                "stylesheet" | "css" => o.types = Some(o.types.unwrap_or(0) | TYPE_STYLESHEET),
                "font" => o.types = Some(o.types.unwrap_or(0) | TYPE_FONT),
                "subdocument" | "sub_frame" | "frame" => {
                    o.types = Some(o.types.unwrap_or(0) | TYPE_SUB_FRAME)
                }
                "document" | "doc" | "main_frame" => {
                    o.types = Some(o.types.unwrap_or(0) | TYPE_MAIN_FRAME)
                }
                "xmlhttprequest" | "xhr" => o.types = Some(o.types.unwrap_or(0) | TYPE_XHR),
                "websocket" => o.types = Some(o.types.unwrap_or(0) | TYPE_WEBSOCKET),
                "media" => o.types = Some(o.types.unwrap_or(0) | TYPE_MEDIA),
                "ping" | "beacon" => o.types = Some(o.types.unwrap_or(0) | TYPE_PING),
                "popup" => o.types = Some(o.types.unwrap_or(0) | TYPE_POPUP),
                "object" | "object-subrequest" | "other" => {
                    o.types = Some(o.types.unwrap_or(0) | TYPE_OTHER)
                }
                "badfilter" | "sitekey" | "generichide" | "elemhide" | "genericblock"
                | "inline-script" | "inline-font" | "ghide" | "shide" | "empty" | "mp4"
                | "content" | "network" => saw_unsupported = true,
                _ => saw_unsupported = true,
            }
        }
    }
    if saw_unsupported { None } else { Some(o) }
}

/// uBO procedural cosmetic operators (not CSS): such filters are dropped.
const PROCEDURAL: &[&str] = &[
    ":has-text(",
    ":xpath(",
    ":matches-css",
    ":matches-attr(",
    ":matches-media(",
    ":matches-path(",
    ":matches-prop(",
    ":upward(",
    ":min-text-length(",
    ":others(",
    ":watch-attr(",
    ":remove(",
    ":style(",
    ":nth-ancestor(",
    ":contains(",
    ":-abp-",
];

fn parse_cosmetic(line: &str) -> Option<Cosmetic> {
    let allow = line.contains("#@#");
    let marker = if allow { "#@#" } else { "##" };
    let pos = line.find(marker)?;
    let hosts = &line[..pos];
    let selector = &line[pos + marker.len()..];
    // The selector goes into a stylesheet as `selector { display: none }`;
    // braces would let it close that rule and add rules of its own.
    if selector.trim().is_empty()
        || selector.contains(['{', '}', '\n', '\r'])
        || selector.contains("/*")
        || PROCEDURAL.iter().any(|p| selector.contains(p))
    {
        return None;
    }
    let mut host_list: Vec<DomainSel> = Vec::new();
    for h in hosts.split(',') {
        let h = h.trim();
        let (neg, raw) = match h.strip_prefix('~') {
            Some(rest) => (true, rest),
            None => (false, h),
        };
        let raw = raw.strip_prefix("*.").unwrap_or(raw);
        if !raw.is_empty() {
            host_list.push(DomainSel {
                host: raw.to_lowercase(),
                neg,
            });
        }
    }
    Some(Cosmetic {
        hosts: host_list,
        selector: selector.trim().to_string(),
        allow,
    })
}

// ---- JS emission helpers ----

fn bit(allow: bool, important: bool) -> u32 {
    (allow as u32) | ((important as u32) << 1)
}

/// `s` as a JavaScript string literal. Control characters and the U+2028 /
/// U+2029 line separators must be escaped too: one raw line break is a
/// syntax error that silently disables the whole page script.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn token_regex(toks: &[Tok], left_anchor: bool, right_anchor: bool) -> String {
    let mut out = String::new();
    if left_anchor {
        out.push('^');
    }
    for t in toks {
        match t {
            Tok::Lit(s) => out.push_str(&escape_regex(s)),
            Tok::Sep => out.push_str("(?:[^A-Za-z0-9_.%~-]|$)"),
            Tok::Any => out.push_str(".*"),
        }
    }
    if right_anchor {
        out.push('$');
    }
    out
}

fn escape_regex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' | '$' | '^' | '.' | '|' | '?' | '*' | '+' | '(' | ')' | '[' | ']' | '{' | '}'
            | '/' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// The page-script body, compiled from uBlock's `contentscript.js` +
/// `cosmetic-filtering.js` model: per-document interception of fetch/XHR/
/// beacon/src/iframe/script loads plus element-hiding `<style>` injection.
/// `__HOST_RULES__` is an object mapping hostname -> `[[regex|null, typeMask,
/// flags], ...]` (flags: 1 allow, 2 important); `__URL_RULES__` an array of
/// the same entries for non-host-anchored filters; `__GEN_CSS__`/
/// `__SCOPED_CSS__` carry the `##` selectors. Type masks match TYPE_*.
/// uBO "scriptlets" for YouTube, as plain JavaScript. YouTube describes its
/// ads inside the player data (`adPlacements`, `playerAds`, `adSlots`), so
/// network rules cannot remove them; uBO prunes those fields before the player
/// reads them (`set-constant ytInitialPlayerResponse.*`, `json-prune`,
/// `json-prune-fetch-response`, `json-prune-xhr-response`). The same is done
/// here, plus a runtime skipper for ads that are not described in the data
/// (server-side inserted ones): it mutes, speeds through and skips them.
const YT_SCRIPTLET: &str = r#"try{
if(/(^|\.)(youtube\.com|youtube-nocookie\.com|youtubekids\.com)$/.test(host)){
  const KEYS=['adPlacements','playerAds','adSlots'];
  const API=/\/youtubei\/v1\/(player|next|browse|search|get_watch|reel\/)|\/watch\?|\/playlist\?list=/;
  const jp=JSON.parse;
  const hasKey=s=>typeof s==='string'&&(s.indexOf('"adPlacements"')>=0||s.indexOf('"playerAds"')>=0||s.indexOf('"adSlots"')>=0);
  const isAdItem=e=>{try{
    if(!e||typeof e!=='object')return false;
    if(e.adSlotRenderer)return true;
    const c=e.richItemRenderer&&e.richItemRenderer.content;
    if(c&&(c.adSlotRenderer||c.promotedSparklesWebRenderer))return true;
    const a=e.command&&e.command.reelWatchEndpoint&&e.command.reelWatchEndpoint.adClientParams;
    return !!(a&&a.isAd);
  }catch(_){return false}};
  const prune=(o,d)=>{
    if(!o||typeof o!=='object'||d>9)return o;
    if(Array.isArray(o)){
      for(let i=o.length-1;i>=0;i--){if(isAdItem(o[i]))o.splice(i,1);else prune(o[i],d+1)}
      return o;
    }
    for(const k of KEYS){if(k in o){try{delete o[k]}catch(_){try{o[k]=undefined}catch(__){}}}}
    for(const k in o){const v=o[k];if(v&&typeof v==='object')prune(v,d+1)}
    return o;
  };
  const pruneText=t=>{try{return JSON.stringify(prune(jp(t),0))}catch(_){return t}};

  /* set-constant: the page assigns the initial player response from an inline
     script; prune it as it is stored. */
  try{
    let v=window.ytInitialPlayerResponse;
    Object.defineProperty(window,'ytInitialPlayerResponse',{configurable:true,enumerable:true,
      get(){return v},set(x){v=(x&&typeof x==='object')?prune(x,0):x}});
  }catch(_){}

  /* json-prune: every JSON.parse result that carries ad fields. */
  try{
    JSON.parse=function(t){
      const r=jp.apply(this,arguments);
      try{if(r&&typeof r==='object'&&hasKey(t))prune(r,0)}catch(_){}
      return r;
    };
  }catch(_){}

  /* json-prune-fetch-response */
  try{
    const of=window.fetch;
    if(of)window.fetch=function(input){
      let url='';
      try{url=typeof input==='string'?input:((input&&input.url)||String(input))}catch(_){}
      const p=of.apply(this,arguments);
      if(!API.test(url))return p;
      return p.then(async resp=>{
        try{
          const text=await resp.clone().text();
          if(!hasKey(text))return resp;
          return new Response(pruneText(text),{status:resp.status,statusText:resp.statusText,headers:resp.headers});
        }catch(_){return resp}
      });
    };
  }catch(_){}

  /* json-prune-xhr-response */
  try{
    const xo=XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open=function(m,u){
      try{this.__bcYt=API.test(String(u))}catch(_){}
      return xo.apply(this,arguments);
    };
    for(const prop of ['response','responseText']){
      const d=Object.getOwnPropertyDescriptor(XMLHttpRequest.prototype,prop);
      if(!d||!d.get)continue;
      Object.defineProperty(XMLHttpRequest.prototype,prop,{configurable:true,enumerable:d.enumerable,
        get(){
          const v=d.get.call(this);
          try{
            if(this.__bcYt&&this.readyState===4){
              if(typeof v==='string'&&hasKey(v))return this['__bc_'+prop]||(this['__bc_'+prop]=pruneText(v));
              if(v&&typeof v==='object')prune(v,0);
            }
          }catch(_){}
          return v;
        }});
    }
  }catch(_){}

  /* Runtime skipper for ads that are not in the data (server-side inserted):
     mute, run the ad to its end and press Skip, then restore the player. */
  let saved=null;
  const tick=()=>{try{
    const p=document.getElementById('movie_player')||document.querySelector('.html5-video-player');
    const v=document.querySelector('video.html5-main-video')||document.querySelector('video');
    const ad=!!p&&(p.classList.contains('ad-showing')||p.classList.contains('ad-interrupting'));
    if(ad&&v){
      if(!saved)saved={m:v.muted,r:v.playbackRate};
      v.muted=true;
      const sk=document.querySelector('.ytp-skip-ad-button,.ytp-ad-skip-button,.ytp-ad-skip-button-modern');
      if(sk)sk.click();
      if(isFinite(v.duration)&&v.duration>0&&v.currentTime<v.duration-0.1){
        v.playbackRate=16;
        try{v.currentTime=Math.max(0,v.duration-0.05)}catch(_){}
      }
    }else if(saved&&v){
      v.muted=saved.m;v.playbackRate=saved.r;saved=null;
    }
    const x=document.querySelector('.ytp-ad-overlay-close-button,.ytp-ad-overlay-slot-close-button');
    if(x)x.click();
    const w=document.querySelector('ytd-enforcement-message-view-model');
    if(w){const dlg=w.closest('tp-yt-paper-dialog')||w;dlg.remove();if(v&&v.paused)v.play().catch(()=>{})}
  }catch(_){}};
  setInterval(tick,250);
}
}catch(e){}
"#;

const JS_HOOK: &str = r#"(function(){
const NET=__NET__;
const M={sub:2,script:4,image:8,sheet:16,font:32,xhr:64,ws:128,media:256,ping:512,other:1024,popup:2048};
const host=String(location.hostname||'').toLowerCase();
const hostMatch=(h,d)=>h===d||h.endsWith('.'+d);
let isTop=true;try{isTop=window.top===window}catch(e){isTop=false}

/* ---- element hiding (top document) ----
   Never touches document.head/documentElement directly: at document-creation
   time neither exists yet. A constructable stylesheet needs neither (and is
   exempt from the page's CSP); the <style> fallback waits for the root. */
try{
  if(isTop){
    const G=__GEN_CSS__,S=__SCOPED_CSS__;
    const applies=hs=>{let hasPos=false,pos=false;for(const e of hs){const m=hostMatch(host,e[0]);if(e[1]){if(m)return false}else{hasPos=true;if(m)pos=true}}return !hasPos||pos};
    const hide=new Set(G),allow=new Set();
    for(const e of S){if(!applies(e[1]))continue;if(e[0])allow.add(e[2]);else hide.add(e[2])}
    const sels=[];hide.forEach(x=>{if(!allow.has(x))sels.push(x)});
    if(sels.length){
      const css=sels.map(x=>x+'{display:none !important}').join('\n');
      let done=false;
      try{
        if(typeof CSSStyleSheet==='function'&&'adoptedStyleSheets' in document){
          const sh=new CSSStyleSheet();sh.replaceSync(css);
          document.adoptedStyleSheets=[...document.adoptedStyleSheets,sh];done=true;
        }
      }catch(e){}
      if(!done){
        const put=()=>{const st=document.createElement('style');st.textContent=css;(document.head||document.documentElement).appendChild(st)};
        if(document.documentElement)put();
        else{const mo=new MutationObserver(()=>{if(document.documentElement){mo.disconnect();put()}});mo.observe(document,{childList:true})}
      }
    }
  }
}catch(e){}

/* ---- scriptlets (site-specific, independent of network blocking) ---- */
__SCRIPTLETS__

/* ---- network blocking (only when the host does not intercept natively) ---- */
if(!NET)return;
const H=__HOST_RULES__;
const N=__URL_RULES__;
/* Regexes compile on first use: a big list has tens of thousands of rules and
   a page only ever touches a handful of them. */
const re=r=>{if(typeof r[0]==='string'){try{r[0]=new RegExp(r[0])}catch(e){r[0]=null}}return r[0]};
const abs=u=>{try{return new URL(String(u),location.href).href}catch(e){return ''}};
const decide=(u,t)=>{
  let un,l,tail;
  try{
    const x=new URL(String(u));
    if(x.protocol!=='http:'&&x.protocol!=='https:')return 0;
    un=x.hostname.toLowerCase();l=x.href.toLowerCase();
    const at=l.indexOf(un,l.indexOf('//')+2);
    tail=at>=0?l.slice(at+un.length):'';
  }catch(e){return 0}
  const third=!(un===host||un.endsWith('.'+host)||host.endsWith('.'+un));
  let block=false,allow=false,imp=false;
  const test=r=>{
    if((r[1]&t)===0||!re(r))return false;
    if(r[3]===1&&third)return false;
    if(r[3]===2&&!third)return false;
    if(r[4]){
      let neg=false,hasPos=false,pos=false;
      for(const d of r[4]){const m=hostMatch(host,d[0]);if(d[1]){if(m)neg=true}else{hasPos=true;if(m)pos=true}}
      if((neg&&!pos)||(hasPos&&!pos))return false;
    }
    return true;
  };
  const hit=r=>{if(r[2]&2)imp=true;if(r[2]&1)allow=true;else block=true};
  const parts=un.split('.');
  for(let i=0;i<parts.length;i++){
    const rs=H[parts.slice(i).join('.')];
    if(!rs)continue;
    for(const r of rs)if(test(r)&&r[0].test(tail))hit(r);
  }
  for(const r of N)if(test(r)&&r[0].test(l))hit(r);
  return block&&(imp||!allow)?1:0;
};
const tagType=el=>{
  const t=(el.tagName||'').toLowerCase();
  if(t==='script')return M.script;
  if(t==='iframe'||t==='frame')return M.sub;
  if(t==='img'||t==='input')return M.image;
  if(t==='link')return /stylesheet/i.test(el.rel||'')?M.sheet:M.other;
  if(t==='audio'||t==='video'||t==='source'||t==='embed'||t==='object'||t==='track')return M.media;
  return M.other;
};
const intercept=(proto,attr)=>{
  try{
    const d=Object.getOwnPropertyDescriptor(proto,attr);
    if(!d||!d.set)return;
    const set=d.set;
    Object.defineProperty(proto,attr,{configurable:true,enumerable:d.enumerable,get:d.get,
      set:function(v){
        try{const a=abs(v);if(a&&decide(a,tagType(this))===1)return}catch(e){}
        set.call(this,v);
      }});
  }catch(e){}
};
intercept(HTMLImageElement.prototype,'src');
intercept(HTMLScriptElement.prototype,'src');
intercept(HTMLIFrameElement.prototype,'src');
intercept(HTMLEmbedElement.prototype,'src');
intercept(HTMLObjectElement.prototype,'data');
intercept(HTMLLinkElement.prototype,'href');
intercept(HTMLAudioElement.prototype,'src');
intercept(HTMLVideoElement.prototype,'src');
intercept(HTMLSourceElement.prototype,'src');
try{
  const oSA=Element.prototype.setAttribute;
  Element.prototype.setAttribute=function(n,v){
    try{
      const k=String(n).toLowerCase();
      if(k==='src'||k==='href'||k==='data'){
        const t=tagType(this);
        if(t!==M.other){const a=abs(v);if(a&&decide(a,t)===1)return}
      }
    }catch(e){}
    return oSA.apply(this,arguments);
  };
}catch(e){}
try{
  const origFetch=window.fetch;
  if(origFetch)window.fetch=function(input){
    try{
      const u=typeof input==='string'?input:(input&&input.url?input.url:String(input));
      if(u&&decide(abs(u),M.xhr)===1)return Promise.reject(new TypeError('Blocked by BrowseCraft'));
    }catch(e){}
    return origFetch.apply(this,arguments);
  };
}catch(e){}
try{
  const origOpen=XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open=function(method,u){
    try{
      if(u&&decide(abs(u),M.xhr)===1){
        const a=Array.prototype.slice.call(arguments);a[1]='about:blank';
        return origOpen.apply(this,a);
      }
    }catch(e){}
    return origOpen.apply(this,arguments);
  };
}catch(e){}
try{
  if(navigator.sendBeacon){
    const sb=navigator.sendBeacon.bind(navigator);
    navigator.sendBeacon=function(u){
      try{if(u&&decide(abs(u),M.ping)===1)return false}catch(e){}
      return sb.apply(navigator,arguments);
    };
  }
}catch(e){}
/* Backstop for tags the HTML parser inserts: the setters above never see
   those, so remove blocked ones as they appear (the native interceptor is the
   reliable path; this only runs where there is none). */
try{
  const ATTR={SCRIPT:'src',IMG:'src',IFRAME:'src',FRAME:'src',EMBED:'src',OBJECT:'data',AUDIO:'src',VIDEO:'src',SOURCE:'src',TRACK:'src',LINK:'href'};
  const check=n=>{
    const a=ATTR[n.tagName];if(!a)return;
    const v=n.getAttribute(a);
    if(v&&decide(abs(v),tagType(n))===1){try{n.removeAttribute(a);n.remove()}catch(e){}}
  };
  const scan=n=>{
    if(!n||n.nodeType!==1)return;
    check(n);
    if(n.querySelectorAll)n.querySelectorAll('script[src],img[src],iframe[src],link[href],embed[src],object[data],source[src],video[src],audio[src]').forEach(check);
  };
  new MutationObserver(ms=>{for(const m of ms)m.addedNodes.forEach(scan)}).observe(document,{childList:true,subtree:true});
}catch(e){}
})();"#;

// ---- Built-in starter list ----

const START_LIST: &str = r#"! BrowseCraft starter list (subset of uBlock Origin defaults)
! Hostname-anchored network filters.
||doubleclick.net^
||googlesyndication.com^
||googletagmanager.com^
||google-analytics.com^
||googleadservices.com^
||adservice.google.com^
||adnxs.com^
||adzerk.net^
||adsrvr.org^
||criteo.com^
||criteo.net^
||mathtag.com^
||openx.net^
||pubmatic.com^
||rubiconproject.com^
||taboola.com^
||outbrain.com^
||quantserve.com^
||scorecardresearch.com^
||adroll.com^
||bidswitch.net^
||cpmstar.com^
||media.net^
||sharethrough.com^
||yieldmo.com^
||adsafeprotected.com^
||moatads.com^
||addthis.com^
||addthisedge.com^
||hotjar.com^
||mixpanel.com^
||segment.io^
||segment.com^
||facebook.com/tr^
||connect.facebook.net^
! Path-specific ad slots (wildcards/separators).
||example-adserver.com^
||ads.example.com/banner/^
|https://ad.doubleclick.net^
! Element-hiding filters (##).
##.adsbygoogle
##div[class*="ad-banner"]
##ins.adsbygoogle
##[id^=google_ads]
##.ad-slot
##.ad-container
##.advertisement
##[aria-label="Ad"]
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_script_strings_are_always_valid_javascript() {
        assert_eq!(quote("a\\b\"c"), r#""a\\b\"c""#);
        assert_eq!(
            quote("x\ry\nz\u{2028}\u{2029}\u{7}"),
            r#""x\ry\nz\u2028\u2029\u0007""#
        );
        // A list with classic Mac (CR-only) line endings is read line by line.
        let ab = blocker(&["||ads.example^\r||track.example^\r##.banner"]);
        assert_eq!(
            decide(&ab, "https://track.example/", TYPE_ALL, "x.com"),
            Decision::Block
        );
        assert_eq!(ab.cosmetic_selectors("x.com"), [".banner"]);
        // Selectors with braces could add rules of their own: dropped.
        let ab = blocker(&["##a{}body{display:none}", "##.ok"]);
        assert_eq!(ab.cosmetic_selectors("x.com"), [".ok"]);
    }

    fn blocker(lists: &[&str]) -> AdBlocker {
        let mut ab = AdBlocker::empty();
        for l in lists {
            ab.add_filter_list("t", l);
        }
        ab
    }

    fn decide(ab: &AdBlocker, url: &str, types: u32, doc: &str) -> Decision {
        let host = host_of(url).unwrap_or_default();
        ab.matches_network(url, &host, types, doc)
    }

    /// The original backtracking matcher, kept as the reference that the
    /// linear one must agree with.
    fn match_tokens_reference(hay: &str, toks: &[Tok], at: usize, scan: bool, right: bool) -> bool {
        let end = hay.len();
        if toks.is_empty() {
            return !right || at >= end;
        }
        if scan {
            // Search the first token forward from `at` (unanchored patterns).
            return match &toks[0] {
                Tok::Lit(s) => {
                    let mut k = at;
                    while let Some(rel) = hay.get(k..end).and_then(|rest| rest.find(s.as_str())) {
                        let p = k + rel;
                        if match_tokens_reference(hay, &toks[1..], p + s.len(), false, right) {
                            return true;
                        }
                        if s.is_empty() {
                            break;
                        }
                        k = p + s.len().max(1);
                        if k > end {
                            break;
                        }
                    }
                    false
                }
                Tok::Sep => {
                    for k in at..=end {
                        if k < end && !is_sep(hay.as_bytes()[k] as char) {
                            continue;
                        }
                        if match_tokens(
                            hay,
                            &toks[1..],
                            if k < end { k + 1 } else { k },
                            false,
                            right,
                        ) {
                            return true;
                        }
                    }
                    false
                }
                Tok::Any => match_tokens_reference(hay, toks, at, false, right),
            };
        }
        match &toks[0] {
            Tok::Lit(s) => {
                hay.get(at..at + s.len()) == Some(s.as_str())
                    && match_tokens_reference(hay, &toks[1..], at + s.len(), false, right)
            }
            Tok::Sep => {
                if at == end {
                    return match_tokens_reference(hay, &toks[1..], end, false, right);
                }
                hay.as_bytes().get(at).is_some_and(|&b| is_sep(b as char))
                    && match_tokens_reference(hay, &toks[1..], at + 1, false, right)
            }
            Tok::Any => {
                if toks.len() == 1 {
                    return true; // a wildcard consumes everything up to end-of-URL
                }
                match &toks[1] {
                    Tok::Lit(s) => {
                        let mut k = at;
                        while let Some(rel) = hay.get(k..end).and_then(|rest| rest.find(s.as_str()))
                        {
                            let p = k + rel;
                            if match_tokens_reference(hay, &toks[2..], p + s.len(), false, right) {
                                return true;
                            }
                            if s.is_empty() {
                                break;
                            }
                            k = p + s.len().max(1);
                            if k > end {
                                break;
                            }
                        }
                        false
                    }
                    Tok::Sep => {
                        for k in at..=end {
                            if k < end && !is_sep(hay.as_bytes()[k] as char) {
                                continue;
                            }
                            if match_tokens(
                                hay,
                                &toks[2..],
                                if k < end { k + 1 } else { k },
                                false,
                                right,
                            ) {
                                return true;
                            }
                        }
                        false
                    }
                    Tok::Any => {
                        // Consecutive `*` can't happen: parse collapses them.
                        false
                    }
                }
            }
        }
    }

    #[test]
    fn linear_matcher_agrees_with_backtracking_reference() {
        let mut seed = 0x9e37_79b9_u32;
        let mut next = |n: u32| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed % n
        };
        let alphabet = ["a", "b", "/", "?", "ab", "ba"];
        for _ in 0..20_000 {
            let mut toks: Vec<Tok> = Vec::new();
            for _ in 0..1 + next(5) {
                let tok = match next(4) {
                    0 => Tok::Sep,
                    1 if !matches!(toks.last(), Some(Tok::Any)) => Tok::Any,
                    _ => Tok::Lit(alphabet[next(6) as usize].to_string()),
                };
                toks.push(tok);
            }
            let hay: String = (0..next(12))
                .map(|_| ["a", "b", "/", "?"][next(4) as usize])
                .collect();
            for at in [0, 1, hay.len()] {
                let at = at.min(hay.len());
                for (scan, right) in [(false, false), (false, true), (true, false), (true, true)] {
                    assert_eq!(
                        match_tokens(&hay, &toks, at, scan, right),
                        match_tokens_reference(&hay, &toks, at, scan, right),
                        "{toks:?} {hay:?} at={at} scan={scan} right={right}"
                    );
                }
            }
        }
    }

    #[test]
    fn many_wildcards_on_a_long_url_are_fast() {
        let ab = blocker(&["a*a*a*a*a*a*a*b|"]);
        let url = format!("https://example.com/{}", "a".repeat(20_000));
        let start = std::time::Instant::now();
        assert_eq!(decide(&ab, &url, TYPE_ALL, "x.com"), Decision::Neutral);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(
            decide(&ab, &(url + "b"), TYPE_ALL, "x.com"),
            Decision::Block
        );
    }

    #[test]
    fn hostname_anchor_uses_the_parsed_host() {
        let ab = blocker(&["||doubleclick.net^", "||ads.example^$document"]);
        for url in [
            // An IDN typed as Unicode: the punycode host is longer than the
            // raw one, which used to index past the end of the URL.
            "http://ä.doubleclick.net/",
            "http://ä.doubleclick.net",
            "http://ä.doubleclick.net:8080/x",
            // `user@` used to shift the host offset and skip the match.
            "https://x@doubleclick.net/",
            "https://user:pw@sub.doubleclick.net/ad.js",
            "HTTPS://DOUBLECLICK.NET/X",
        ] {
            assert_eq!(
                decide(&ab, url, TYPE_ALL, "other.com"),
                Decision::Block,
                "{url}"
            );
        }
        assert_eq!(
            decide(&ab, "https://doubleclick.network/", TYPE_ALL, "other.com"),
            Decision::Neutral
        );
        assert!(blocked_page_with(&ab, "http://ä.ads.example/"));
        assert!(blocked_page_with(&ab, "https://u@ads.example/"));
    }

    fn blocked_page_with(ab: &AdBlocker, url: &str) -> bool {
        decide(ab, url, TYPE_DOC_PAGE, "") == Decision::Block
    }

    #[test]
    fn hostname_anchor_blocks_host_and_subdomains() {
        let ab = blocker(&["||example.com^"]);
        assert_eq!(
            decide(&ab, "http://example.com/page", TYPE_ALL, "other.com"),
            Decision::Block
        );
        assert_eq!(
            decide(&ab, "http://sub.example.com/page", TYPE_ALL, "other.com"),
            Decision::Block
        );
        assert_eq!(
            decide(&ab, "http://example.net/page", TYPE_ALL, "other.com"),
            Decision::Neutral
        );
        assert_eq!(
            decide(&ab, "http://sub.example.net/page", TYPE_ALL, "other.com"),
            Decision::Neutral
        );
    }

    #[test]
    fn exception_overrides_block() {
        let ab = blocker(&["||example.com^", "@@||example.com^"]);
        assert_eq!(
            decide(&ab, "http://example.com/x", TYPE_ALL, "other.com"),
            Decision::Allow
        );
        let ab = blocker(&["||example.com^", "@@||example.com/keep^"]);
        assert_eq!(
            decide(&ab, "http://example.com/x", TYPE_ALL, "other.com"),
            Decision::Block
        );
    }

    #[test]
    fn plain_and_anchors() {
        let ab = blocker(&[
            "|https://exact.example.com/|",
            "|https://prefix.example.com",
            "suffix.example.com/end|",
            "/ad.js",
        ]);
        // URLs are matched as `url` serializes them, so an empty path is `/`.
        assert_eq!(
            decide(&ab, "https://exact.example.com", TYPE_ALL, "x.com"),
            Decision::Block
        );
        assert_eq!(
            decide(&ab, "https://exact.example.com/", TYPE_ALL, "x.com"),
            Decision::Block
        );
        assert_eq!(
            decide(&ab, "https://exact.example.com/x", TYPE_ALL, "x.com"),
            Decision::Neutral
        );
        assert_eq!(
            decide(&ab, "https://prefix.example.com/path", TYPE_ALL, "x.com"),
            Decision::Block
        );
        assert_eq!(
            decide(
                &ab,
                "https://x.com/suffix.example.com/end",
                TYPE_ALL,
                "x.com"
            ),
            Decision::Block
        );
        assert_eq!(
            decide(
                &ab,
                "https://x.com/suffix.example.com/endmore",
                TYPE_ALL,
                "x.com"
            ),
            Decision::Neutral
        );
        assert_eq!(
            decide(&ab, "https://x.com/ad.js?x=1", TYPE_ALL, "x.com"),
            Decision::Block
        );
    }

    #[test]
    fn wildcards_and_separators() {
        let ab = blocker(&["||example.com/*/banner^", "|https://cdn.example.com/*/*.js"]);
        // `/banner/x` lacks a second `/` before "banner" — the `/*/` needs one.
        assert_eq!(
            decide(&ab, "https://example.com/banner/x", TYPE_ALL, "x.com"),
            Decision::Neutral
        );
        // '.' is not a separator (like '-', '_', '%'), so `banner^` does not
        // match "banner.js" either.
        assert_eq!(
            decide(&ab, "https://example.com/a/banner.js", TYPE_ALL, "x.com"),
            Decision::Neutral
        );
        // `/a/banner/x` has the `/` + anything + `/banner` shape, and `/x`
        // after "banner" is a separator -> match.
        assert_eq!(
            decide(&ab, "https://example.com/a/banner/x", TYPE_ALL, "x.com"),
            Decision::Block
        );
        assert_eq!(
            decide(&ab, "https://cdn.example.com/1/2/app.js", TYPE_ALL, "x.com"),
            Decision::Block
        );
    }

    #[test]
    fn separator_is_not_alphanumeric() {
        let ab = blocker(&["||example.com^"]);
        // A port colon and a path slash both terminate the authority.
        assert_eq!(
            decide(&ab, "http://example.com:8080/x", TYPE_ALL, "x.com"),
            Decision::Block
        );
        // 'fooexample.com' is a different hostname (not a suffix boundary).
        assert_eq!(
            decide(&ab, "http://fooexample.com/x", TYPE_ALL, "x.com"),
            Decision::Neutral
        );
    }

    #[test]
    fn type_options() {
        let ab = blocker(&["||example.com^$script"]);
        assert_eq!(
            decide(&ab, "http://example.com/x.js", TYPE_SCRIPT, "x.com"),
            Decision::Block
        );
        assert_eq!(
            decide(&ab, "http://example.com/x.js", TYPE_IMAGE, "x.com"),
            Decision::Neutral
        );
        let ab = blocker(&["||example.com^$script,image"]);
        assert_eq!(
            decide(&ab, "http://example.com/x", TYPE_IMAGE, "x.com"),
            Decision::Block
        );
    }

    #[test]
    fn third_party_option() {
        let ab = blocker(&["||tracker.com^$third-party"]);
        assert_eq!(
            decide(&ab, "http://tracker.com/p", TYPE_ALL, "site.com"),
            Decision::Block
        );
        assert_eq!(
            decide(&ab, "http://tracker.com/p", TYPE_ALL, "tracker.com"),
            Decision::Neutral
        );
        // Subdomain of the document counts as first-party.
        assert_eq!(
            decide(&ab, "http://tracker.com/p", TYPE_ALL, "cdn.tracker.com"),
            Decision::Neutral
        );
    }

    #[test]
    fn domain_option() {
        let ab = blocker(&["||tracker.com^$domain=shop.com"]);
        assert_eq!(
            decide(&ab, "http://tracker.com/p", TYPE_ALL, "shop.com"),
            Decision::Block
        );
        assert_eq!(
            decide(&ab, "http://tracker.com/p", TYPE_ALL, "other.com"),
            Decision::Neutral
        );
        let ab = blocker(&["||tracker.com^$domain=~shop.com"]);
        assert_eq!(
            decide(&ab, "http://tracker.com/p", TYPE_ALL, "shop.com"),
            Decision::Neutral
        );
        assert_eq!(
            decide(&ab, "http://tracker.com/p", TYPE_ALL, "other.com"),
            Decision::Block
        );
    }

    #[test]
    fn unsupported_options_are_dropped_safely() {
        let ab = blocker(&[
            "||example.com^$redirect=noop.js",
            "||example.com^$csp=script-src 'none'",
            "||example.com^",
        ]);
        // Only the plain rule remains active.
        assert_eq!(
            decide(&ab, "http://example.com/x", TYPE_ALL, "x.com"),
            Decision::Block
        );
    }

    #[test]
    fn cosmetic_filters_and_exceptions() {
        let ab = blocker(&[
            "##.ad",
            "example.com##.ads-box",
            "other.com###banner",
            "example.com#@#.ads-box",
        ]);
        assert_eq!(ab.cosmetic_selectors("example.com"), vec![".ad"]);
        assert_eq!(ab.cosmetic_selectors("other.com"), vec![".ad", "#banner"]);
        assert_eq!(ab.cosmetic_selectors(""), vec![".ad"]);
    }

    #[test]
    fn cosmetic_css_output() {
        let ab = blocker(&["##.ad"]);
        assert_eq!(
            ab.engine_cosmetic_css("x.com"),
            ".ad { display: none !important }"
        );
    }

    #[test]
    fn ignored_lines() {
        let ab = blocker(&["! a comment", "", "[Adblock Plus 2.0]", "##", "@@", "||"]);
        assert_eq!(ab.rules.len(), 0);
    }

    #[test]
    fn page_script_carries_rules() {
        let ab = blocker(&["||doubleclick.net^", "##.adsbygoogle"]);
        let js = ab.page_script();
        assert!(js.contains("doubleclick"));
        assert!(js.contains("adsbygoogle"));
        assert!(js.contains("HTMLImageElement"));
        assert!(js.contains("window.fetch"));
    }

    #[test]
    fn important_overrides_exception() {
        let ab = blocker(&["||example.com^$important", "@@||example.com^"]);
        assert_eq!(
            decide(&ab, "http://example.com/x", TYPE_ALL, "x.com"),
            Decision::Block
        );
    }

    #[test]
    fn token_index_agrees_with_scanning_every_rule() {
        let list = "/ads/banner\n&adunit=\n/pagead/*\n|http://tracker.example/pixel|\n||cdn.example^*/adframe.\n\
            -banner-ad-\n/track^\nads*.gif\n.com/ad/\n@@/ads/banner/ok\n/AdServer/$script\n*/tag.js$third-party\n\
            /x.php?ad=\nban$image\n/wp-content/ads/\n_300x250.\n://ads.\n|https://z.test/a|";
        let ab = blocker(&[list]);
        let mut naive = blocker(&[list]);
        naive.by_token.clear();
        naive.loose = naive.plain.clone();
        assert!(!ab.by_token.is_empty(), "some rules must be indexed");
        let urls = [
            "https://site.test/ads/banner/1.png",
            "https://site.test/ads/banner/ok.png",
            "https://site.test/x?a=1&adunit=5",
            "https://site.test/pagead/ads?x=1",
            "http://tracker.example/pixel",
            "http://tracker.example/pixel2",
            "https://cdn.example/v1/adframe.html",
            "https://site.test/css/my-banner-ad-top.png",
            "https://site.test/track",
            "https://site.test/track/x",
            "https://site.test/tracker",
            "https://adsense.test/img.gif",
            "https://foo.com/ad/1",
            "https://foo.com/AdServer/x.js",
            "https://other.test/lib/tag.js",
            "https://site.test/x.php?ad=1",
            "https://site.test/banner.png",
            "https://site.test/wp-content/ads/a.png",
            "https://site.test/img_300x250.jpg",
            "https://ads.site.test/",
            "https://z.test/a",
            "https://z.test/ab",
            "https://clean.test/index.html",
        ];
        for u in urls {
            for (types, doc) in [(TYPE_ALL, "site.test"), (TYPE_SCRIPT, "other.test"), (TYPE_IMAGE, "")] {
                assert_eq!(decide(&ab, u, types, doc), decide(&naive, u, types, doc), "{u} {types}");
            }
        }
        assert_eq!(decide(&ab, "https://site.test/ads/banner/1.png", TYPE_ALL, "site.test"), Decision::Block);
    }

    #[test]
    fn unsafe_tokens_are_not_indexed() {
        // `ads*` and `*ads` could match inside a longer word: no safe token.
        let ab = blocker(&["ads*.gif", "*banner"]);
        assert!(ab.by_token.is_empty());
        assert_eq!(ab.loose.len(), 2);
        assert_eq!(decide(&ab, "https://x.test/roads1.gif", TYPE_ALL, "x.test"), Decision::Block);
    }

    #[test]
    fn cosmetic_host_lists_keep_every_host() {
        let ab = blocker(&["a.com,b.com##.x", "~c.com##.y", "d.com,~e.d.com##.z"]);
        assert_eq!(ab.cosmetic_selectors("a.com"), vec![".x", ".y"]);
        assert_eq!(ab.cosmetic_selectors("b.com"), vec![".x", ".y"]);
        assert_eq!(ab.cosmetic_selectors("c.com"), Vec::<String>::new());
        assert_eq!(ab.cosmetic_selectors("d.com"), vec![".y", ".z"]);
        assert_eq!(ab.cosmetic_selectors("e.d.com"), vec![".y"]);
    }

    #[test]
    fn procedural_and_scriptlet_filters_are_dropped() {
        let ab = blocker(&[
            "a.com##+js(aopr, x)",
            "a.com##div:has-text(Ad)",
            "a.com#?#div:has(> .x)",
            "a.com##.ok",
        ]);
        assert_eq!(ab.cosmetic_selectors("a.com"), vec![".ok"]);
    }

    #[test]
    fn page_script_with_native_network_has_no_rule_tables() {
        let ab = blocker(&["||doubleclick.net^", "/ads/banner", "##.adsbygoogle"]);
        let js = ab.page_script_with(false);
        assert!(!js.contains("doubleclick"));
        assert!(!js.contains("banner"));
        assert!(js.contains("adsbygoogle"));
        assert!(js.contains("const NET=false"));
        assert!(ab.page_script_with(true).contains("doubleclick"));
        // No unreplaced placeholders.
        for t in ["__SCRIPTLETS__", "__NET__", "__HOST_RULES__", "__URL_RULES__", "__GEN_CSS__", "__SCOPED_CSS__"] {
            assert!(!js.contains(t) && !ab.page_script().contains(t), "{t}");
        }
    }

    #[test]
    fn js_rules_carry_party_and_domain_options() {
        let ab = blocker(&["||ads.test^$third-party,domain=a.com|~b.a.com"]);
        let js = ab.page_script();
        assert!(js.contains("a.com"));
        assert!(js.contains(",2,"), "third-party flag");
    }

    #[test]
    fn blocked_request_uses_the_document_host() {
        let mut ab = AdBlocker::empty();
        ab.add_filter_list("t", "||ads.test^$third-party");
        assert_eq!(decide(&ab, "https://ads.test/x.js", TYPE_SCRIPT, "ads.test"), Decision::Neutral);
        assert_eq!(decide(&ab, "https://ads.test/x.js", TYPE_SCRIPT, "news.test"), Decision::Block);
    }

    #[test]
    fn youtube_scriptlet_is_in_the_page_script_in_both_modes() {
        let ab = blocker(&["##.ad"]);
        for network in [true, false] {
            let js = ab.page_script_with(network);
            assert!(js.contains("adPlacements") && js.contains("ytInitialPlayerResponse"));
            assert!(js.contains("ad-showing"));
            assert!(!js.contains("__SCRIPTLETS__"));
        }
    }

    #[test]
    fn bundled_youtube_list_hides_player_ads_and_blocks_ad_stats() {
        let ab = blocker(&[include_str!("../filters/youtube.txt")]);
        let sel = ab.cosmetic_selectors("www.youtube.com");
        for want in ["ytd-ad-slot-renderer", "#player-ads", "#masthead-ad"] {
            assert!(sel.iter().any(|s| s == want), "{want}");
        }
        assert!(ab.cosmetic_selectors("example.com").is_empty());
        assert_eq!(
            decide(&ab, "https://www.youtube.com/api/stats/ads?ver=2", TYPE_XHR, "www.youtube.com"),
            Decision::Block
        );
        assert_eq!(
            decide(&ab, "https://www.youtube.com/youtubei/v1/player?key=1", TYPE_XHR, "www.youtube.com"),
            Decision::Neutral
        );
    }
}
