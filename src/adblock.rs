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
//! 2. WebView2 pages: `page_script()` emits the equivalent content-script
//!    (uBO's `contentscript.js`) that cancels matching fetch/XHR/beacon/img/
//!    script/iframe loads and injects a `<style>` for `##` selectors.
//! 3. Engine pages: `engine_cosmetic_css()` returns a stylesheet applying
//!    `display: none` through the built-in selector engine.
//!
//! Limitations vs uBO: no regex filters (`/re/`), no `$redirect`/`$csp`/
//! `$replace`/scriptlet/procedural filters (rules using them are dropped, never
//! over-blocking), `$domain=` is honoured only Rust-side, and third-party is
//! computed with a hostname-suffix check instead of the PSL.

use std::collections::HashMap;
use std::path::Path;
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
    /// None = applies to every document.
    host: Option<DomainSel>,
    selector: String,
    allow: bool,
}

pub struct AdBlocker {
    /// Rules keyed by the hostname they are anchored on, for fast lookup.
    by_host: HashMap<String, Vec<usize>>,
    plain: Vec<usize>,
    rules: Vec<NetRule>,
    cosmetic: Vec<Cosmetic>,
}

// ---- Public API ----

impl AdBlocker {
    pub fn empty() -> Self {
        AdBlocker {
            by_host: HashMap::new(),
            plain: Vec::new(),
            rules: Vec::new(),
            cosmetic: Vec::new(),
        }
    }

    pub fn add_filter_list(&mut self, _name: &str, content: &str) {
        for line in content.lines() {
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

        for &i in &self.plain {
            consider(&self.rules[i]);
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

    /// Hide selectors applying to `doc_host`, exceptions (`#@#`) subtracted.
    pub fn cosmetic_selectors(&self, doc_host: &str) -> Vec<String> {
        let mut hide: Vec<String> = Vec::new();
        let mut allow: Vec<String> = Vec::new();
        for c in &self.cosmetic {
            let applied = match &c.host {
                None => true,
                Some(h) => domain_sel_matches(h, doc_host),
            };
            if applied {
                if c.allow {
                    allow.push(c.selector.clone());
                } else {
                    hide.push(c.selector.clone());
                }
            }
        }
        hide.into_iter()
            .filter(|s| !allow.iter().any(|a| a == s))
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

    /// The WebView2 content script: cancels matching network requests and
    /// injects element-hiding CSS, in every document (ads live in iframes, so
    /// it runs in subframes too, like uBO's content script). The four `__*__`
    /// tokens are replaced with the compiled rules.
    pub fn page_script(&self) -> String {
        let mut url_rules: Vec<String> = Vec::new();
        for &i in &self.plain {
            let r = &self.rules[i];
            if !r.options.domains.is_empty() {
                continue; // `domain=` is enforced Rust-side only.
            }
            if !r.pattern.host.is_empty() {
                continue;
            }
            let re = token_regex(
                &r.pattern.toks,
                r.pattern.left_anchor,
                r.pattern.right_anchor,
            );
            url_rules.push(format!(
                "[{},{},{}]",
                quote(&re),
                r.options.types.unwrap_or(TYPE_ALL),
                bit(r.allow, r.options.important)
            ));
        }
        url_rules.sort();
        let mut host_map: Vec<String> = Vec::new();
        for (host, indices) in &self.by_host {
            let mut entries = Vec::new();
            let mut ok = true;
            for &i in indices {
                let r = &self.rules[i];
                if !r.options.domains.is_empty() {
                    ok = false;
                    break;
                }
                let re = token_regex(&r.pattern.toks, false, r.pattern.right_anchor);
                entries.push(format!(
                    "[{},{},{}]",
                    quote(&re),
                    r.options.types.unwrap_or(TYPE_ALL),
                    bit(r.allow, r.options.important)
                ));
            }
            if ok && !entries.is_empty() {
                host_map.push(format!("{}:[{}],", quote(host), entries.join(",")));
            }
        }
        host_map.sort();
        let mut generic_css: Vec<String> = Vec::new();
        for sel in self.cosmetic_selectors("") {
            generic_css.push(quote(&sel));
        }
        let mut scoped: Vec<(String, Vec<String>)> = Vec::new();
        for c in &self.cosmetic {
            if let (Some(h), false) = (&c.host, c.allow) {
                let key = if h.neg {
                    format!("~{}", h.host)
                } else {
                    h.host.clone()
                };
                if let Some(entry) = scoped.iter_mut().find(|entry| entry.0 == key) {
                    if !entry.1.contains(&c.selector) {
                        entry.1.push(c.selector.clone());
                    }
                } else {
                    scoped.push((key, vec![c.selector.clone()]));
                }
            }
        }
        scoped.sort_by(|a, b| a.0.cmp(&b.0));
        let mut scoped_js = Vec::new();
        for (host, sels) in scoped {
            if !sels.is_empty() {
                scoped_js.push(format!(
                    "[{},[{}]]",
                    quote(&host),
                    sels.iter().map(|s| quote(s)).collect::<Vec<_>>().join(",")
                ));
            }
        }
        JS_HOOK
            .replace("__HOST_RULES__", &format!("{{{}}}", host_map.join("")))
            .replace("__URL_RULES__", &format!("[{}]", url_rules.join(",")))
            .replace("__GEN_CSS__", &format!("[{}]", generic_css.join(",")))
            .replace("__SCOPED_CSS__", &format!("[{}]", scoped_js.join(",")))
    }

    fn add_filter_line(&mut self, line: &str) {
        if line.is_empty()
            || line.starts_with('!')
            || line.starts_with('[')
            || line.starts_with("#!#")
        {
            return;
        }
        if line.contains("##") || line.contains("#@#") {
            if let Some(c) = parse_cosmetic(line) {
                self.cosmetic.push(c);
            }
            return;
        }
        if line.contains("#?#") || line.contains("#$#") || line.contains("##+js") {
            return;
        }
        if let Some(rule) = parse_network(line) {
            let index = self.rules.len();
            self.rules.push(rule);
            if self.rules[index].pattern.host.is_empty() {
                self.plain.push(index);
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
    for dir in dirs {
        for text in load_directory(&dir.join("filters")).into_iter().flatten() {
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

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
}

// ---- Matching helpers ----

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

fn parse_cosmetic(line: &str) -> Option<Cosmetic> {
    let allow = line.contains("#@#");
    let marker = if allow { "#@#" } else { "##" };
    let pos = line.find(marker)?;
    let hosts = &line[..pos];
    let selector = &line[pos + marker.len()..];
    if selector.is_empty() {
        return None;
    }
    let host = if hosts.trim().is_empty() {
        None
    } else {
        let mut first: Option<DomainSel> = None;
        for h in hosts.split(',') {
            let h = h.trim();
            if h.is_empty() {
                continue;
            }
            let (neg, raw) = if let Some(rest) = h.strip_prefix('~') {
                (true, rest)
            } else {
                (false, h)
            };
            let raw = raw.strip_prefix("*.").unwrap_or(raw);
            if raw.is_empty() {
                continue;
            }
            if first.is_none() {
                first = Some(DomainSel {
                    host: raw.to_lowercase(),
                    neg,
                });
            }
        }
        first
    };
    Some(Cosmetic {
        host,
        selector: selector.to_string(),
        allow,
    })
}

// ---- JS emission helpers ----

fn bit(allow: bool, important: bool) -> u32 {
    (allow as u32) | ((important as u32) << 1)
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
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
const JS_HOOK: &str = r#"(function(){
const M={sub:2,script:4,image:8,sheet:16,font:32,xhr:64,ws:128,media:256,ping:512,other:1024,popup:2048};
const H=Object.assign(Object.create(null),__HOST_RULES__);
const N=__URL_RULES__||[];
const R=s=>{try{return s===null?null:new RegExp(s)}catch(e){return null}};
for(const k in H){H[k].forEach(e=>{e[0]=R(e[0]);});}
for(const e of N){e[0]=R(e[0]);}
const abs=u=>{try{return new URL(u,location.href).href}catch(e){return ''}};
const decide=(u,t)=>{
  const l=String(u).toLowerCase();
  let un='';try{un=new URL(String(u)).hostname.toLowerCase()}catch(e){return 0}
  let block=false,allow=false,imp=false;
  if(un){
    const start=l.indexOf(un);
    const tail=start>=0?l.slice(start+un.length):'';
    const parts=un.split('.');
    for(let i=0;i<parts.length;i++){
      const rs=H[parts.slice(i).join('.')];
      if(!rs)continue;
      for(const r of rs){
        const re=r[0],mask=r[1],flags=r[2];
        if((mask&t)===0)continue;
        if(!(re===null?true:re.test(tail)))continue;
        if(flags&2)imp=true;
        if(flags&1)allow=true;else block=true;
      }
    }
  }
  for(const r of N){
    const re=r[0],mask=r[1],flags=r[2];
    if((mask&t)===0)continue;
    if(!re.test(l))continue;
    if(flags&2)imp=true;
    if(flags&1)allow=true;else block=true;
  }
  if(block && (imp || !allow))return 1;
  return 0;
};
{const g=__GEN_CSS__||[],s=__SCOPED_CSS__||[];
 if(g.length||s.length){
   const h=location.hostname.toLowerCase();
   let css=g.slice();
   for(const x of s){
     const host=x[0].startsWith('~')?x[0].slice(1):x[0];
     const neg=x[0].startsWith('~');
     const mm=neg?(h!==host&&!h.endsWith('.'+host)):(h===host||h.endsWith('.'+host));
     if(mm)css=css.concat(x[1]);
   }
   if(css.length){
     const st=document.createElement('style');
     st.textContent=css.map(x=>x+'{display:none !important}').join('');
     (document.head||document.documentElement).appendChild(st);
   }
 }}
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
  const d=Object.getOwnPropertyDescriptor(proto,attr);
  if(!d||!d.set)return;
  const set=d.set;
  Object.defineProperty(proto,attr,{
    configurable:true,enumerable:d.enumerable,get:d.get,
    set:function(v){
      const a=abs(v);
      if(a&&decide(a,tagType(this))===1)return;
      set.call(this,v);
    }
  });
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
const oSA=Element.prototype.setAttribute;
Element.prototype.setAttribute=function(n,v){
  if(n==='src'||n==='href'||n==='data'){
    const t=tagType(this);
    if(t!==M.other){
      const a=abs(v);
      if(a&&decide(a,t)===1)return;
    }
  }
  return oSA.call(this,n,v);
};
const origFetch=window.fetch;
if(origFetch){
  window.fetch=function(input,init){
    const u=typeof input==='string'?input:(input&&input.url?input.url:'');
    if(u&&decide(abs(u),M.xhr)===1)return Promise.reject(new TypeError('Blocked by BrowseCraft'));
    return origFetch.apply(this,arguments);
  };
}
const origOpen=XMLHttpRequest.prototype.open;
XMLHttpRequest.prototype.open=function(method,u){
  if(u&&decide(abs(u),M.xhr)===1)return;
  return origOpen.apply(this,arguments);
};
if(navigator.sendBeacon){
  const sb=navigator.sendBeacon.bind(navigator);
  navigator.sendBeacon=function(u,data){
    if(u&&decide(abs(u),M.ping)===1)return false;
    return sb.apply(navigator,arguments);
  };
}
const oCE=document.createElement;
document.createElement=function(tag){
  const el=oCE.call(this,tag);
  return el;
};
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
}
