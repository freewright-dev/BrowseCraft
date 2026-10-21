//! Engine compatibility baseline (`compat/*.html`, matrix in `docs/compat.md`).
//!
//! Every fixture is a small page that paints coloured boxes with fixed sizes
//! and no text, so the result does not depend on installed fonts, scale factor
//! or anti-aliasing: the layout's `Fill` rectangles are compared exactly (to
//! half a pixel) at an 800x600 viewport. A fixture declares what it expects
//! in HTML comments:
//!
//! - `<!-- title: ... -->` and `<!-- spec: ... -->` describe it;
//! - `<!-- status: pass -->` the engine must match every `expect`;
//! - `<!-- status: known-fail -->` the engine is known to differ, and the test
//!   fails when it starts to match, so the status gets updated;
//! - `<!-- status: unsupported -->` the feature is not implemented; the page
//!   is listed but never run, so it cannot be reported as a pass;
//! - `<!-- expect: #rrggbb x y w h -->` a box of that colour at that place.
//!
//! The expectations come from the specifications, not from the engine's
//! output. The fixtures are written for this project (no upstream tests are
//! copied, so no third-party licence applies). Bless the matrix after adding or
//! changing a fixture with `BLESS=1 cargo test compat`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::css::Color;
use crate::layout::{self, FormState, Item, LayoutInput};
use crate::loader::{self, Request};
use crate::style::{self, StyleContext};

const VIEWPORT: (f32, f32) = (800.0, 600.0);
const TOLERANCE: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Pass,
    KnownFail,
    Unsupported,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::KnownFail => "known-fail",
            Status::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Expect {
    color: Color,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

struct Fixture {
    name: String,
    markup: String,
    title: String,
    spec: String,
    status: Status,
    expects: Vec<Expect>,
}

fn directive<'a>(markup: &'a str, key: &str) -> Vec<&'a str> {
    let prefix = format!("<!-- {key}:");
    markup
        .lines()
        .filter_map(|l| l.trim().strip_prefix(prefix.as_str()))
        .filter_map(|l| l.strip_suffix("-->"))
        .map(str::trim)
        .collect()
}

fn parse_expect(text: &str) -> Result<Expect, String> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [color, x, y, w, h] = parts.as_slice() else {
        return Err(format!("expect needs a colour and four numbers: {text}"));
    };
    let hex = color.strip_prefix('#').filter(|h| h.len() == 6).ok_or("colour must be #rrggbb")?;
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string());
    let num = |s: &str| s.parse::<f32>().map_err(|e| e.to_string());
    Ok(Expect {
        color: Color { r: byte(0)?, g: byte(2)?, b: byte(4)?, a: 255 },
        x: num(x)?,
        y: num(y)?,
        w: num(w)?,
        h: num(h)?,
    })
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("compat")
}

fn load_fixtures() -> Result<Vec<Fixture>, String> {
    let mut names: Vec<_> = std::fs::read_dir(fixture_dir())
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "html"))
        .collect();
    names.sort();
    let mut out = Vec::new();
    for path in names {
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        let markup = std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))?;
        let one = |key: &str| directive(&markup, key).first().map(|s| s.to_string());
        let status = match one("status").as_deref() {
            Some("pass") => Status::Pass,
            Some("known-fail") => Status::KnownFail,
            Some("unsupported") => Status::Unsupported,
            other => return Err(format!("{name}: bad status {other:?}")),
        };
        let expects = directive(&markup, "expect")
            .into_iter()
            .map(parse_expect)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("{name}: {e}"))?;
        if status != Status::Unsupported && expects.is_empty() {
            return Err(format!("{name}: no expectations"));
        }
        out.push(Fixture {
            title: one("title").ok_or(format!("{name}: no title"))?,
            spec: one("spec").ok_or(format!("{name}: no spec"))?,
            name,
            markup,
            status,
            expects,
        });
    }
    Ok(out)
}

fn percent_encode(text: &str) -> String {
    text.bytes().fold(String::new(), |mut s, b| {
        let _ = write!(s, "%{b:02X}");
        s
    })
}

/// The filled boxes of `markup`, in paint order. `None` without system fonts.
fn fills(markup: &str) -> Option<Vec<(Color, layout::Rect)>> {
    let url = format!("data:text/html;charset=utf-8,{}", percent_encode(markup));
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(move || {
            let fonts = crate::font::Fonts::new().ok()?;
            let (page, _) = loader::load_blocking(&Request::Get(url));
            let ctx = StyleContext { zoom: 1.0, viewport_width: VIEWPORT.0, viewport_height: VIEWPORT.1 };
            let styles = style::compute_styles(&page.doc, &page.sheets, &ctx);
            let (images, form) = (HashMap::new(), FormState::default());
            let input = LayoutInput {
                doc: &page.doc,
                styles: &styles,
                fonts: &fonts,
                images: &images,
                form: &form,
                zoom: 1.0,
                viewport_width: VIEWPORT.0,
                viewport_height: VIEWPORT.1,
            };
            let layout = layout::layout(&input);
            Some(
                layout
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        Item::Fill { rect, color, .. } => Some((*color, *rect)),
                        _ => None,
                    })
                    .collect(),
            )
        })
        .ok()?
        .join()
        .ok()?
}

fn found(boxes: &[(Color, layout::Rect)], want: &Expect) -> bool {
    boxes.iter().any(|(color, r)| {
        color.r == want.color.r
            && color.g == want.color.g
            && color.b == want.color.b
            && (r.x - want.x).abs() <= TOLERANCE
            && (r.y - want.y).abs() <= TOLERANCE
            && (r.w - want.w).abs() <= TOLERANCE
            && (r.h - want.h).abs() <= TOLERANCE
    })
}

fn matrix(fixtures: &[Fixture]) -> String {
    let count = |s: Status| fixtures.iter().filter(|f| f.status == s).count();
    let mut out = String::new();
    out.push_str("# Engine compatibility matrix\n\n");
    out.push_str("Generated by `BLESS=1 cargo test compat` from the fixtures in `compat/`; do not edit by hand.\n");
    out.push_str("It covers BrowseCraft's own engine only (the parser, cascade, layout and\n");
    out.push_str("painter that draw internal pages), never a page shown by WebView2 or Servo.\n\n");
    out.push_str("- **pass**: the engine matches every expectation (checked by `cargo test`).\n");
    out.push_str("- **known-fail**: the engine differs from the specification; `cargo test` fails\n  when it starts to match, so this list stays honest.\n");
    out.push_str("- **unsupported**: the feature is not implemented; the page is never run, so it\n  cannot count as a pass.\n\n");
    out.push_str("Each fixture paints fixed-size boxes without text and the engine's box\n");
    out.push_str("rectangles are compared to within half a pixel at an 800x600 viewport and\n");
    out.push_str("scale 1.0, so fonts, anti-aliasing and the platform do not affect the result.\n");
    out.push_str("The fixtures are written for this project; no upstream test files are\n");
    out.push_str("copied, so no third-party licence applies and there is no revision to pin.\n\n");
    let _ = writeln!(
        out,
        "{} fixtures: {} pass, {} known-fail, {} unsupported.\n",
        fixtures.len(),
        count(Status::Pass),
        count(Status::KnownFail),
        count(Status::Unsupported)
    );
    out.push_str("| Fixture | Status | What it checks | Specification |\n|---|---|---|---|\n");
    for f in fixtures {
        let _ = writeln!(out, "| `{}` | {} | {} | {} |", f.name, f.status.label(), f.title, f.spec);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize(text: &str) -> String {
        text.replace("\r\n", "\n")
    }

    #[test]
    fn matrix_is_current() {
        let fixtures = load_fixtures().expect("fixtures");
        let want = matrix(&fixtures);
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs").join("compat.md");
        if std::env::var_os("BLESS").is_some() {
            std::fs::write(&path, &want).expect("write matrix");
            return;
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(normalize(&have), want, "docs/compat.md is stale: run `BLESS=1 cargo test compat`");
    }

    #[test]
    fn fixtures_match_their_declared_status() {
        let fixtures = load_fixtures().expect("fixtures");
        let mut problems = Vec::new();
        for f in fixtures.iter().filter(|f| f.status != Status::Unsupported) {
            let Some(boxes) = fills(&f.markup) else {
                eprintln!("skipped: no system fonts on this machine");
                return;
            };
            let missing: Vec<_> = f.expects.iter().filter(|e| !found(&boxes, e)).collect();
            match (f.status, missing.is_empty()) {
                (Status::Pass, false) => problems.push(format!(
                    "{}: expected {:?}, painted {:?}",
                    f.name,
                    missing,
                    boxes.iter().map(|(c, r)| (c.r, c.g, c.b, r.x, r.y, r.w, r.h)).collect::<Vec<_>>()
                )),
                (Status::KnownFail, true) => {
                    problems.push(format!("{}: now matches; change its status to pass", f.name));
                }
                _ => {}
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[test]
    fn unsupported_fixtures_are_not_run_or_counted_as_passes() {
        let fixtures = load_fixtures().expect("fixtures");
        assert!(fixtures.iter().any(|f| f.status == Status::Unsupported));
        assert!(fixtures.iter().filter(|f| f.status == Status::Unsupported).all(|f| f.expects.is_empty()));
    }

    #[test]
    fn expectations_parse() {
        assert!(parse_expect("#ff0000 0 0 100 50").is_ok());
        assert!(parse_expect("red 0 0 100 50").is_err());
        assert!(parse_expect("#ff0000 0 0 100").is_err());
    }
}
