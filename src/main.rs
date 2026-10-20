#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! BrowseCraft: a small web browser with its own rendering engine.
//!
//! Pipeline: `net` fetches bytes, `html` parses them into a `dom`, `css`
//! parses stylesheets, `style` runs the cascade, `layout` builds a display
//! list and `paint` rasterizes it. `browser` is the window and UI around it.

mod adblock;
mod address;
mod bookmarks;
mod browser;
mod capture;
mod css;
mod dom;
mod egui_chrome;
mod flags;
mod font;
mod headless;
mod history;
mod html;
mod icons;
mod internal;
mod layout;
mod loader;
mod net;
mod paint;
mod passwords;
mod servo_pane;
mod storage;
mod style;
mod textfield;
#[cfg(test)]
mod test_http;
mod vault;
#[cfg(windows)]
mod webview_network;

use winit::event_loop::EventLoop;

fn main() {
    flags::load();
    bookmarks::load();
    passwords::load();
    internal::set_dark(flags::get().theme != flags::Theme::Light);
    let fonts = match font::Fonts::new() {
        Ok(fonts) => fonts,
        Err(e) => {
            eprintln!("browsecraft: {e}");
            std::process::exit(1);
        }
    };
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--screenshot") {
        let (Some(out), Some(url)) = (args.get(2), args.get(3)) else {
            eprintln!("usage: browsecraft --screenshot <out.png> <url> [width]");
            std::process::exit(2);
        };
        let width = match args.get(4) {
            None => 1200,
            Some(w) => match w.parse::<u32>() {
                Ok(w) if (1..=headless::MAX_WIDTH).contains(&w) => w,
                _ => {
                    eprintln!(
                        "browsecraft: the width must be a number from 1 to {}",
                        headless::MAX_WIDTH
                    );
                    std::process::exit(2);
                }
            },
        };
        if let Err(e) = headless::screenshot(&fonts, &address::resolve(url), out, width) {
            eprintln!("browsecraft: {e}");
            std::process::exit(1);
        }
        return;
    }
    let start_url = std::env::args()
        .nth(1)
        .map(|arg| address::resolve(&arg))
        .unwrap_or_else(|| browser::HOME_URL.to_string());

    let event_loop = match EventLoop::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("browsecraft: failed to start the event loop: {e}");
            std::process::exit(1);
        }
    };
    let mut app = browser::Browser::new(event_loop.create_proxy(), fonts, start_url);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("browsecraft: {e}");
    }
}
