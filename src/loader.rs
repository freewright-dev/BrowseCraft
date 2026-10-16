//! Loads a page on a background thread: the document, its stylesheets, then
//! its images, reporting each stage back to the UI thread.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use url::Url;

use crate::css::{self, Stylesheet};
use crate::dom::{Document, NodeKind};
use crate::html;
use crate::net::{self, escape_html};
use crate::paint::Image;

const HOME_HTML: &str = include_str!("home.html");
const ACCEPT_HTML: &str = "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.8";
const MAX_IMAGE_PIXELS: u64 = 40_000_000;
const IMAGE_WORKERS: usize = 6;

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

        let queue = Mutex::new(images);
        std::thread::scope(|scope| {
            for _ in 0..IMAGE_WORKERS {
                scope.spawn(|| {
                    while live() {
                        let Some((src, url)) = queue.lock().ok().and_then(|mut q| q.pop()) else {
                            break;
                        };
                        if let Some(image) = fetch_image(&url)
                            && live()
                        {
                            send(generation, LoadEvent::Image { src, image });
                        }
                    }
                });
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
    let images = std::thread::scope(|scope| {
        let handles: Vec<_> = sources
            .into_iter()
            .map(|(src, url)| scope.spawn(move || fetch_image(&url).map(|img| (src, img))))
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .collect()
    });
    (page, images)
}

fn load_page(request: &Request) -> Page {
    let raw = request.url();
    if let Some(inner) = raw.strip_prefix("about:blocked:") {
        return ad_blocked_page(raw, inner);
    }
    if let Some(inner) = raw.strip_prefix("view-source:") {
        return view_source(inner);
    }
    if let Some(query) = raw.strip_prefix("browsecraft://flags") {
        // The flags page submits to itself with the new values in the query.
        let pairs = url::form_urlencoded::parse(query.strip_prefix('?').unwrap_or("").as_bytes());
        crate::flags::apply_query(pairs.map(|(k, v)| (k.into_owned(), v.into_owned())));
        let source = crate::flags::page_html();
        return internal_page(raw, &source);
    }
    if raw.starts_with("browsecraft://history") {
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
        Err(e) => return error_page(raw, &format!("Browsecraft could not load this page: {e}.")),
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
         <p style=\"color:#57606a\">Browsecraft's ad filter stopped this address from loading.</p>\
         <p style=\"color:#57606a;font-size:14px;word-break:break-all\">{}</p></div>",
        escape_html(url)
    );
    let mut page = internal_page(raw, &source);
    page.url = url.to_string();
    page
}

fn view_source(inner: &str) -> Page {
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
        .unwrap_or_else(|| url.clone())
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
    let host_ref = &host;
    let sources = sheet_sources(doc, Some(base));
    std::thread::scope(|scope| {
        let handles: Vec<_> = sources
            .into_iter()
            .map(|source| {
                scope.spawn(move || match source {
                    SheetSource::Inline(text, media) => {
                        let sheet = css::parse_stylesheet(&text);
                        with_imports(sheet, base, media.as_deref(), 0, host_ref)
                    }
                    SheetSource::Link(url, media) => {
                        fetch_sheet(&url, media.as_deref(), 0, host_ref)
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

fn fetch_sheet(url: &Url, media: Option<&str>, depth: u32, doc_host: &str) -> Vec<Stylesheet> {
    if crate::adblock::blocked_subresource(url.as_str(), crate::adblock::TYPE_STYLESHEET, doc_host)
    {
        return Vec::new();
    }
    match net::fetch(url, None, "text/css,*/*;q=0.1") {
        Ok(resp) if resp.status < 400 => {
            let text = net::decode_text(&resp.body, &resp.content_type);
            with_imports(
                css::parse_stylesheet(&text),
                &resp.url,
                media,
                depth,
                doc_host,
            )
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
    doc_host: &str,
) -> Vec<Stylesheet> {
    let mut out = Vec::new();
    if depth < 3 {
        for import in &sheet.imports {
            if let Ok(url) = base.join(import) {
                out.extend(fetch_sheet(&url, media, depth + 1, doc_host));
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
            && !crate::adblock::blocked_subresource(url.as_str(), crate::adblock::TYPE_IMAGE, &host)
        {
            out.push((src.to_string(), url));
        }
    }
    // Workers pop from the end; load images in document order.
    out.reverse();
    out
}

fn fetch_image(url: &Url) -> Option<Image> {
    let resp = net::fetch(
        url,
        None,
        "image/webp,image/png,image/jpeg,image/gif,*/*;q=0.5",
    )
    .ok()?;
    if resp.status >= 400 {
        return None;
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(&resp.body))
        .with_guessed_format()
        .ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    if w as u64 * h as u64 > MAX_IMAGE_PIXELS {
        return None;
    }
    let rgba = image::load_from_memory(&resp.body).ok()?.to_rgba8();
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
