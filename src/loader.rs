//! Loads a page on a background thread: the document, its stylesheets, then
//! its images, reporting each stage back to the UI thread.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use url::Url;

use crate::css::{self, Stylesheet};
use crate::dom::{Document, NodeKind};
use crate::html;
use crate::net::{self, escape_html};
use crate::paint::Image;

const HOME_HTML: &str = include_str!("home.html");
const ACCEPT_HTML: &str = "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.8";
/// Decoded images are kept as one `u32` per pixel: 16 Mpx is 64 MB.
const MAX_IMAGE_PIXELS: u64 = 4096 * 4096;
/// All images of one page together: 64 Mpx is 256 MB.
const MAX_PAGE_IMAGE_PIXELS: u64 = 4 * MAX_IMAGE_PIXELS;
const IMAGE_WORKERS: usize = 6;
/// `<style>` and `<link rel=stylesheet>` elements honoured per page.
const MAX_SHEETS: usize = 256;
/// Stylesheet requests per page, `@import`s included.
const MAX_SHEET_FETCHES: usize = 128;
const SHEET_WORKERS: usize = 6;

#[derive(Clone, Debug)]
pub enum Request {
    Get(String),
    /// URL and urlencoded form body.
    Post(String, String),
}

impl Request {
    pub fn url(&self) -> &str {
        match self {
            Request::Get(url) | Request::Post(url, _) => url,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    /// What the address bar shows: the URL after redirects.
    pub url: String,
    pub base: Url,
    pub doc: Document,
    pub sheets: Vec<Stylesheet>,
}

#[derive(Clone, Debug)]
pub enum LoadEvent {
    Page(Page),
    Image { src: String, image: Image },
    Finished,
}

/// Loads `request` in the background. `generation` identifies this load;
/// work stops early once `current` moves on to a newer load.
pub fn spawn(
    request: Request,
    generation: u64,
    current: Arc<AtomicU64>,
    send: impl Fn(u64, LoadEvent) + Send + Sync + 'static,
) {
    std::thread::spawn(move || {
        let live = || current.load(Ordering::SeqCst) == generation;
        let page = load_page(&request);
        let images = image_sources(&page);
        send(generation, LoadEvent::Page(page));

        fetch_images(images, MAX_PAGE_IMAGE_PIXELS, &live, |src, image| {
            if live() {
                send(generation, LoadEvent::Image { src, image });
            }
        });
        if live() {
            send(generation, LoadEvent::Finished);
        }
    });
}

/// Loads a page and all its images on the calling thread.
pub fn load_blocking(request: &Request) -> (Page, Vec<(String, Image)>) {
    let page = load_page(request);
    let sources = image_sources(&page);
    let images = Mutex::new(Vec::new());
    fetch_images(sources, MAX_PAGE_IMAGE_PIXELS, &|| true, |src, image| {
        if let Ok(mut images) = images.lock() {
            images.push((src, image));
        }
    });
    (page, images.into_inner().unwrap_or_default())
}

/// Runs `work` on up to `workers` threads, the calling thread included, so
/// the work still gets done if the OS refuses to start more threads.
fn run_workers(workers: usize, work: impl Fn() + Sync) {
    std::thread::scope(|scope| {
        for _ in 1..workers {
            if std::thread::Builder::new()
                .spawn_scoped(scope, &work)
                .is_err()
            {
                break;
            }
        }
        work();
    });
}

/// Fetches and decodes `sources` (popped from the end) on a few worker
/// threads while `live()` holds, handing each image to `done`. Decoding
/// stops once the page's images would exceed `budget` pixels.
fn fetch_images(
    sources: Vec<(String, Url)>,
    budget: u64,
    live: &(impl Fn() -> bool + Sync),
    done: impl Fn(String, Image) + Sync,
) {
    let queue = Mutex::new(sources);
    let budget = AtomicU64::new(budget);
    run_workers(IMAGE_WORKERS, || {
        while live() {
            let Some((src, url)) = queue.lock().ok().and_then(|mut q| q.pop()) else {
                break;
            };
            if let Some(image) = fetch_image(&url, &budget) {
                done(src, image);
            }
        }
    });
}

/// The URL inside `about:blocked:<url>`, if the ad filter really blocks
/// it. BrowseCraft makes these addresses itself for blocked pages; anyone
/// else (a link, a typed address) could otherwise put any URL, say a bank's,
/// in the address bar over the "blocked" notice.
pub fn blocked_notice_target(raw: &str) -> Option<&str> {
    let inner = raw.strip_prefix("about:blocked:")?;
    let lower = inner.to_ascii_lowercase();
    let web = lower.starts_with("http://") || lower.starts_with("https://");
    (web && crate::adblock::blocked_page(inner, "")).then_some(inner)
}

fn load_page(request: &Request) -> Page {
    let raw = request.url();
    if raw.starts_with("about:blocked:") {
        return match blocked_notice_target(raw) {
            Some(inner) => ad_blocked_page(raw, inner),
            None => error_page(raw, "This address is not valid."),
        };
    }
    if let Some(inner) = raw.strip_prefix("view-source:") {
        return view_source(inner);
    }
    if let Some(query) = internal_query(raw, "browsecraft://flags") {
        // The flags page submits to itself with the new values in the query.
        crate::flags::apply_form(query);
        let source = crate::flags::page_html();
        return internal_page(raw, &source);
    }
    if internal_query(raw, "browsecraft://history").is_some() {
        let source = crate::history::page_html();
        return internal_page(raw, &source);
    }
    match raw {
        "about:home" => return internal_page(raw, HOME_HTML),
        "about:blank" => return internal_page(raw, ""),
        _ => {}
    }
    let url = match Url::parse(raw) {
        Ok(url) => url,
        Err(e) => return error_page(raw, &format!("This address is not valid ({e}).")),
    };
    let form = match request {
        Request::Post(_, body) => Some(body.as_str()),
        Request::Get(_) => None,
    };
    let resp = match net::fetch(&url, form, ACCEPT_HTML) {
        Ok(resp) => resp,
        Err(e) => return error_page(raw, &format!("BrowseCraft could not load this page: {e}.")),
    };

    let mime = resp
        .content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let final_url = resp.url.to_string();
    let source = if mime.starts_with("image/") && mime != "image/svg+xml" {
        format!(
            "<title>{0}</title><body style=\"margin:0;background:#1e1e1e;text-align:center\"><img src=\"{0}\" style=\"max-width:100%\">",
            escape_html(&final_url)
        )
    } else if mime.is_empty()
        || mime.contains("html")
        || mime.contains("xml") && !mime.contains("svg")
    {
        net::decode_text(&resp.body, &resp.content_type)
    } else if mime.starts_with("text/") || mime.contains("json") || mime.contains("javascript") {
        let text = net::decode_text(&resp.body, &resp.content_type);
        format!(
            "<pre style=\"white-space:pre-wrap;margin:8px\">{}</pre>",
            escape_html(&text)
        )
    } else {
        return error_page(
            &final_url,
            &format!(
                "This file can't be displayed ({mime}, {} KB).",
                resp.body.len() / 1024
            ),
        );
    };

    let doc = html::parse(&source);
    let base = document_base(&doc, &resp.url);
    let mut sheets = load_stylesheets(&doc, &base);
    let host = base.host_str().map(str::to_lowercase).unwrap_or_default();
    let cosmetics = crate::adblock::instance().engine_cosmetic_css(&host);
    if !cosmetics.is_empty() {
        sheets.push(css::parse_stylesheet(&cosmetics));
    }
    Page {
        url: final_url,
        base,
        doc,
        sheets,
    }
}

/// The query of `raw` if it is exactly the internal page `page`, optionally
/// followed by `?query` (so `browsecraft://flagsX` is not the flags page).
fn internal_query<'a>(raw: &'a str, page: &str) -> Option<&'a str> {
    match raw.strip_prefix(page)? {
        "" => Some(""),
        rest => rest.strip_prefix('?'),
    }
}

fn internal_page(url: &str, source: &str) -> Page {
    let doc = html::parse(source);
    let base = Url::parse("about:blank").expect("valid url");
    let sheets = inline_sheets(&doc);
    Page {
        url: url.to_string(),
        base,
        doc,
        sheets,
    }
}

pub fn error_page(url: &str, message: &str) -> Page {
    let source = format!(
        "<title>Problem loading page</title>\
         <body style=\"font-family:sans-serif;margin:0;background:#f6f7f9;color:#1f2328\">\
         <div style=\"max-width:640px;margin:80px auto;padding:0 24px\">\
         <h1 style=\"font-size:24px;font-weight:600\">Hmm. We can't reach this page.</h1>\
         <p style=\"color:#57606a\">{}</p>\
         <p style=\"color:#57606a;font-size:14px;word-break:break-all\">{}</p></div>",
        escape_html(message),
        escape_html(url)
    );
    let mut page = internal_page(url, &source);
    page.url = url.to_string();
    page
}

fn ad_blocked_page(raw: &str, url: &str) -> Page {
    let source = format!(
        "<title>Blocked page</title>\
         <body style=\"font-family:sans-serif;margin:0;background:#f6f7f9;color:#1f2328\">\
         <div style=\"max-width:640px;margin:80px auto;padding:0 24px\">\
         <h1 style=\"font-size:24px;font-weight:600\">This page was blocked.</h1>\
         <p style=\"color:#57606a\">BrowseCraft's ad filter stopped this address from loading.</p>\
         <p style=\"color:#57606a;font-size:14px;word-break:break-all\">{}</p></div>",
        escape_html(url)
    );
    let mut page = internal_page(raw, &source);
    page.url = url.to_string();
    page
}

fn view_source(inner: &str) -> Page {
    // The source of a blocked page is not fetched either.
    let lower = inner.to_ascii_lowercase();
    if (lower.starts_with("http://") || lower.starts_with("https://"))
        && crate::adblock::blocked_page(inner, "")
    {
        let mut page = ad_blocked_page(inner, inner);
        page.url = format!("view-source:{inner}");
        return page;
    }
    let source = Url::parse(inner)
        .map_err(|e| e.to_string())
        .and_then(|url| net::fetch(&url, None, ACCEPT_HTML))
        .map(|resp| net::decode_text(&resp.body, &resp.content_type));
    let body = match source {
        Ok(text) => text,
        Err(e) => format!("Could not load the source: {e}"),
    };
    let html = format!(
        "<title>view-source:{0}</title><pre style=\"white-space:pre-wrap;margin:8px;font-size:13px\">{1}</pre>",
        escape_html(inner),
        escape_html(&body)
    );
    let mut page = internal_page(inner, &html);
    page.url = format!("view-source:{inner}");
    page
}

fn document_base(doc: &Document, url: &Url) -> Url {
    doc.find_tag("base")
        .and_then(|b| doc.element(b))
        .and_then(|e| e.attr("href"))
        .and_then(|href| url.join(href).ok())
        .filter(|base| may_fetch(url, base))
        .unwrap_or_else(|| url.clone())
}

/// Whether a document (or stylesheet) at `from` may load `url`. Only local
/// files may load other local files: a web page must not read the disk.
fn may_fetch(from: &Url, url: &Url) -> bool {
    url.scheme() != "file" || from.scheme() == "file"
}

enum SheetSource {
    Inline(String, Option<String>),
    Link(Url, Option<String>),
}

fn sheet_sources(doc: &Document, base: Option<&Url>) -> Vec<SheetSource> {
    let mut out = Vec::new();
    for node in doc.descendants(Document::ROOT) {
        let Some(el) = doc.element(node) else {
            continue;
        };
        let media = el
            .attr("media")
            .map(str::trim)
            .filter(|m| !m.is_empty() && *m != "all")
            .map(str::to_string);
        if media
            .as_deref()
            .is_some_and(|m| m.eq_ignore_ascii_case("print"))
        {
            continue;
        }
        match el.tag.as_str() {
            "style" => out.push(SheetSource::Inline(doc.text_content(node), media)),
            "link" => {
                let rel = el.attr("rel").unwrap_or("").to_ascii_lowercase();
                let is_sheet =
                    rel.split_whitespace().any(|r| r == "stylesheet") && !rel.contains("alternate");
                if let (true, Some(base), Some(href)) = (is_sheet, base, el.attr("href"))
                    && let Ok(url) = base.join(href.trim())
                    && may_fetch(base, &url)
                {
                    out.push(SheetSource::Link(url, media));
                }
            }
            _ => {}
        }
    }
    out
}

fn with_media(mut sheet: Stylesheet, media: Option<&str>) -> Stylesheet {
    if let Some(media) = media {
        for rule in &mut sheet.rules {
            rule.media = Some(match &rule.media {
                Some(inner) => format!("{media}&&{inner}"),
                None => media.to_string(),
            });
        }
    }
    sheet
}

fn inline_sheets(doc: &Document) -> Vec<Stylesheet> {
    sheet_sources(doc, None)
        .into_iter()
        .filter_map(|s| match s {
            SheetSource::Inline(text, media) => {
                Some(with_media(css::parse_stylesheet(&text), media.as_deref()))
            }
            SheetSource::Link(..) => None,
        })
        .collect()
}

/// Fetches and parses every stylesheet, in document order, fetching linked
/// sheets in parallel.
fn load_stylesheets(doc: &Document, base: &Url) -> Vec<Stylesheet> {
    let host = base.host_str().map(str::to_lowercase).unwrap_or_default();
    let mut sources = sheet_sources(doc, Some(base));
    // The same stylesheet linked twice loads once.
    let mut linked = HashSet::new();
    sources.retain(|s| match s {
        SheetSource::Link(url, _) => linked.insert(url.clone()),
        SheetSource::Inline(..) => true,
    });
    sources.truncate(MAX_SHEETS);

    let fetches = AtomicUsize::new(MAX_SHEET_FETCHES);
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Vec<Stylesheet>>> = sources.iter().map(|_| Mutex::default()).collect();
    run_workers(SHEET_WORKERS.min(sources.len()), || {
        loop {
            let i = next.fetch_add(1, Ordering::SeqCst);
            let Some(source) = sources.get(i) else {
                break;
            };
            let mut ctx = SheetContext {
                doc_host: &host,
                fetches: &fetches,
                seen: HashSet::new(),
            };
            let sheets = match source {
                SheetSource::Inline(text, media) => {
                    let sheet = css::parse_stylesheet(text);
                    with_imports(sheet, base, media.as_deref(), 0, &mut ctx)
                }
                SheetSource::Link(url, media) => fetch_sheet(url, media.as_deref(), 0, &mut ctx),
            };
            if let Ok(mut slot) = results[i].lock() {
                *slot = sheets;
            }
        }
    });
    // Document order, whichever worker finished first.
    results
        .into_iter()
        .flat_map(|r| r.into_inner().unwrap_or_default())
        .collect()
}

/// What one `<style>` or `<link>` and its `@import`s may still fetch.
struct SheetContext<'a> {
    doc_host: &'a str,
    /// Stylesheet requests left for the whole page.
    fetches: &'a AtomicUsize,
    /// URLs this sheet's `@import` tree has already fetched, so that `N`
    /// sheets importing the same `N` sheets cost `N` requests, not `N²`.
    seen: HashSet<Url>,
}

impl SheetContext<'_> {
    fn claim(&mut self, url: &Url) -> bool {
        self.seen.insert(url.clone())
            && self
                .fetches
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
    }
}

fn fetch_sheet(
    url: &Url,
    media: Option<&str>,
    depth: u32,
    ctx: &mut SheetContext,
) -> Vec<Stylesheet> {
    if crate::adblock::blocked_subresource(
        url.as_str(),
        crate::adblock::TYPE_STYLESHEET,
        ctx.doc_host,
    ) || !ctx.claim(url)
    {
        return Vec::new();
    }
    match net::fetch(url, None, "text/css,*/*;q=0.1") {
        Ok(resp) if resp.status < 400 => {
            let text = net::decode_text(&resp.body, &resp.content_type);
            with_imports(css::parse_stylesheet(&text), &resp.url, media, depth, ctx)
        }
        _ => Vec::new(),
    }
}

/// Puts `@import`ed sheets before the sheet that imports them.
fn with_imports(
    sheet: Stylesheet,
    base: &Url,
    media: Option<&str>,
    depth: u32,
    ctx: &mut SheetContext,
) -> Vec<Stylesheet> {
    let mut out = Vec::new();
    if depth < 3 {
        for import in &sheet.imports {
            if let Ok(url) = base.join(import)
                && may_fetch(base, &url)
            {
                out.extend(fetch_sheet(&url, media, depth + 1, ctx));
            }
        }
    }
    out.push(with_media(sheet, media));
    out
}

/// Image sources to fetch, as (src attribute, resolved URL). Sources the ad
/// filter blocks are omitted.
fn image_sources(page: &Page) -> Vec<(String, Url)> {
    let doc = &page.doc;
    let host = page
        .base
        .host_str()
        .map(str::to_lowercase)
        .unwrap_or_default();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for node in doc.descendants(Document::ROOT) {
        let NodeKind::Element(el) = &doc.nodes[node].kind else {
            continue;
        };
        let is_image_input = el.tag == "input"
            && el
                .attr("type")
                .is_some_and(|t| t.eq_ignore_ascii_case("image"));
        if el.tag != "img" && !is_image_input {
            continue;
        }
        let Some(src) = el.image_source() else {
            continue;
        };
        if !seen.insert(src.to_string()) {
            continue;
        }
        if let Ok(url) = page.base.join(src)
            && may_fetch(&page.base, &url)
            && !crate::adblock::blocked_subresource(url.as_str(), crate::adblock::TYPE_IMAGE, &host)
        {
            out.push((src.to_string(), url));
        }
    }
    // Workers pop from the end; load images in document order.
    out.reverse();
    out
}

/// Fetches and decodes one image, taking its pixels out of the page's
/// `budget`. Returns `None` for images that are too big or over budget.
fn fetch_image(url: &Url, budget: &AtomicU64) -> Option<Image> {
    let resp = net::fetch(
        url,
        None,
        "image/webp,image/png,image/jpeg,image/gif,*/*;q=0.5",
    )
    .ok()?;
    if resp.status >= 400 {
        return None;
    }
    decode_image(&resp.body, budget)
}

fn decode_image(bytes: &[u8], budget: &AtomicU64) -> Option<Image> {
    let reader = || {
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(16_384);
        limits.max_image_height = Some(16_384);
        // The decoded image in its own format, up to 16 bytes a pixel.
        limits.max_alloc = Some(MAX_IMAGE_PIXELS * 16);
        reader.limits(limits);
        Some(reader)
    };
    let (w, h) = reader()?.into_dimensions().ok()?;
    let area = w as u64 * h as u64;
    if area > MAX_IMAGE_PIXELS
        || budget
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(area)
            })
            .is_err()
    {
        return None;
    }
    let rgba = reader()?.decode().ok()?.to_rgba8();
    let pixels = rgba
        .pixels()
        .map(|p| (p[3] as u32) << 24 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32)
        .collect();
    Some(Image {
        width: rgba.width(),
        height: rgba.height(),
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_source_of_a_blocked_page_is_not_fetched() {
        // filters/basic.txt (loaded from the working directory) blocks this.
        if !crate::adblock::blocked_page("https://doubleclick.net/", "") {
            return;
        }
        let page = load_page(&Request::Get("view-source:https://doubleclick.net/".into()));
        assert_eq!(page.url, "view-source:https://doubleclick.net/");
        assert!(
            page.doc
                .text_content(Document::ROOT)
                .contains("was blocked")
        );
    }
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// Serves CSS on 127.0.0.1: `/a.css` imports `b0`…`b9`, each `bN`
    /// imports `c0`…`c9`, and any other path is a plain rule. Returns the
    /// base URL and a count of requests served.
    fn css_server() -> (Url, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let served = count.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let served = served.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(&stream);
                    let mut request = String::new();
                    reader.read_line(&mut request).unwrap_or(0);
                    let mut line = String::new();
                    while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                        line.clear();
                    }
                    let path = request.split(' ').nth(1).unwrap_or("/").to_string();
                    served.fetch_add(1, Ordering::SeqCst);
                    let imports = |prefix: &str| -> String {
                        (0..10)
                            .map(|i| format!("@import '{prefix}{i}.css';"))
                            .collect()
                    };
                    let body = match path.as_str() {
                        "/a.css" => imports("b"),
                        p if p.starts_with("/b") => imports("c"),
                        _ => "p { color: red }".to_string(),
                    };
                    let _ = write!(
                        &stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/css\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        (base, count)
    }

    #[test]
    fn imports_fetch_each_url_once_per_tree() {
        let (base, count) = css_server();
        // `<link>`ed twice: loads once.
        let doc =
            html::parse(r#"<link rel=stylesheet href=a.css><link rel=stylesheet href=a.css>"#);
        let sheets = load_stylesheets(&doc, &base);
        // a, b0..b9, and c0..c9 once (under b0) instead of 10 times.
        assert_eq!(count.load(Ordering::SeqCst), 21);
        assert_eq!(sheets.len(), 21);
    }

    #[test]
    fn stylesheet_requests_are_capped_per_page() {
        let (base, count) = css_server();
        let markup: String = (0..MAX_SHEET_FETCHES + 50)
            .map(|i| format!("<link rel=stylesheet href=d{i}.css>"))
            .collect();
        let sheets = load_stylesheets(&html::parse(&markup), &base);
        assert_eq!(count.load(Ordering::SeqCst), MAX_SHEET_FETCHES);
        assert_eq!(sheets.len(), MAX_SHEET_FETCHES);
    }

    #[test]
    fn stylesheets_are_capped_and_stay_in_document_order() {
        let markup: String = (0..MAX_SHEETS + 100)
            .map(|i| format!("<style>p{{z-index:{i}}}</style>"))
            .collect();
        let base = Url::parse("about:blank").unwrap();
        let sheets = load_stylesheets(&html::parse(&markup), &base);
        assert_eq!(sheets.len(), MAX_SHEETS);
        for (i, sheet) in sheets.iter().enumerate() {
            assert_eq!(sheet.rules[0].declarations[0].value, i.to_string());
        }
    }

    /// A PNG of `w`×`h` pixels, or just its header when `header_only`.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::new(w, h)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn images_are_capped_per_image_and_per_page() {
        // Only the header is read for an oversized image: patch the IHDR
        // dimensions of a tiny PNG (bytes 16..24) to 5000×5000.
        let mut huge = png(1, 1);
        huge[16..20].copy_from_slice(&5000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&5000u32.to_be_bytes());
        let mut crc = flate2::Crc::new();
        crc.update(&huge[12..29]); // the IHDR chunk type and data
        huge[29..33].copy_from_slice(&crc.sum().to_be_bytes());
        let header = image::ImageReader::new(std::io::Cursor::new(&huge)).with_guessed_format();
        assert_eq!(header.unwrap().into_dimensions().unwrap(), (5000, 5000));
        let budget = AtomicU64::new(u64::MAX);
        assert!(decode_image(&huge, &budget).is_none());
        assert_eq!(budget.load(Ordering::SeqCst), u64::MAX);

        // A page budget of 25k pixels fits two 100×100 images, not three.
        let small = png(100, 100);
        let budget = AtomicU64::new(25_000);
        assert!(decode_image(&small, &budget).is_some());
        assert!(decode_image(&small, &budget).is_some());
        assert!(decode_image(&small, &budget).is_none());
        assert_eq!(budget.load(Ordering::SeqCst), 5_000);
    }

    const MARKUP: &str = r#"<base href="file:///C:/">
        <link rel="stylesheet" href="file:///C:/a.css"><link rel="stylesheet" href="b.css">
        <img src="file:///C:/secret.png"><img src="c.png">"#;

    fn page_at(url: &str) -> Page {
        let doc = html::parse(MARKUP);
        let url = Url::parse(url).unwrap();
        let base = document_base(&doc, &url);
        Page {
            url: url.to_string(),
            base,
            doc,
            sheets: Vec::new(),
        }
    }

    fn links(page: &Page) -> Vec<String> {
        sheet_sources(&page.doc, Some(&page.base))
            .into_iter()
            .filter_map(|s| match s {
                SheetSource::Link(url, _) => Some(url.to_string()),
                SheetSource::Inline(..) => None,
            })
            .collect()
    }

    fn images(page: &Page) -> Vec<String> {
        let mut urls: Vec<_> = image_sources(page)
            .into_iter()
            .map(|(_, u)| u.to_string())
            .collect();
        urls.sort();
        urls
    }

    #[test]
    fn blocked_notices_only_for_blocked_urls() {
        // Nothing in the test build's filter blocks a bank.
        let raw = "about:blocked:https://bank.example/login";
        assert_eq!(blocked_notice_target(raw), None);
        let page = load_page(&Request::Get(raw.into()));
        assert_eq!(page.url, raw);
        assert!(
            !page
                .doc
                .text_content(Document::ROOT)
                .contains("was blocked")
        );
        assert_eq!(blocked_notice_target("about:blocked:javascript:x"), None);
        assert_eq!(blocked_notice_target("https://bank.example/"), None);
    }

    #[test]
    fn web_pages_cannot_load_local_files() {
        let page = page_at("https://example.com/dir/page.html");
        assert_eq!(page.base.as_str(), "https://example.com/dir/page.html");
        assert_eq!(links(&page), ["https://example.com/dir/b.css"]);
        assert_eq!(images(&page), ["https://example.com/dir/c.png"]);

        // A real file, so a blocked @import differs from a failed one.
        let path = std::env::temp_dir().join(format!("browsecraft-{}.css", std::process::id()));
        std::fs::write(&path, "p { color: red }").unwrap();
        let import = format!("@import '{}';", Url::from_file_path(&path).unwrap());
        let imported = |from: &str| {
            let sheet = css::parse_stylesheet(&import);
            let fetches = AtomicUsize::new(MAX_SHEET_FETCHES);
            let mut ctx = SheetContext {
                doc_host: "",
                fetches: &fetches,
                seen: HashSet::new(),
            };
            with_imports(sheet, &Url::parse(from).unwrap(), None, 0, &mut ctx).len()
        };
        assert_eq!(imported("https://example.com/s.css"), 1);
        assert_eq!(imported("file:///D:/site/s.css"), 2);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn local_pages_can_load_local_files() {
        let page = page_at("file:///D:/site/page.html");
        assert_eq!(page.base.as_str(), "file:///C:/");
        assert_eq!(links(&page), ["file:///C:/a.css", "file:///C:/b.css"]);
        assert_eq!(images(&page), ["file:///C:/c.png", "file:///C:/secret.png"]);
    }
}
