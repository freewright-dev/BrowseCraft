#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod address;

use tao::{
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::{Window, WindowBuilder},
};
use wry::{NewWindowResponse, PageLoadEvent, Rect, WebView, WebViewBuilder};

const HOME_URL: &str = "https://duckduckgo.com/";
const TOOLBAR_HEIGHT: f64 = 44.0;
const TOOLBAR_HTML: &str = include_str!("toolbar.html");

/// Forwards browser keyboard shortcuts from web pages to the shell.
const CONTENT_INIT_SCRIPT: &str = r#"
document.addEventListener("keydown", (e) => {
  let cmd = null;
  if (e.key === "F5" || (e.ctrlKey && e.key === "r")) cmd = "reload";
  else if (e.ctrlKey && e.key === "l") cmd = "focus-url";
  else if (e.altKey && e.key === "ArrowLeft") cmd = "back";
  else if (e.altKey && e.key === "ArrowRight") cmd = "forward";
  else if (e.altKey && e.key === "Home") cmd = "home";
  if (cmd) { e.preventDefault(); window.ipc.postMessage(cmd); }
}, true);
"#;

#[derive(Debug)]
enum UserEvent {
    /// A command from the toolbar or a page shortcut.
    Command(String),
    UrlChanged(String),
    Loading(bool),
    TitleChanged(String),
}

fn main() -> wry::Result<()> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("Browsecraft")
        .with_inner_size(LogicalSize::new(1200.0, 800.0))
        .with_min_inner_size(LogicalSize::new(400.0, 200.0))
        .build(&event_loop)
        .expect("failed to create window");

    let start_url = std::env::args()
        .nth(1)
        .map(|arg| address::resolve(&arg))
        .unwrap_or_else(|| HOME_URL.to_string());

    let (toolbar_bounds, content_bounds) = layout(&window);

    let toolbar = WebViewBuilder::new()
        .with_html(TOOLBAR_HTML)
        .with_bounds(toolbar_bounds)
        .with_ipc_handler(command_handler(proxy.clone()))
        .build_as_child(&window)?;

    let content = WebViewBuilder::new()
        .with_url(&start_url)
        .with_bounds(content_bounds)
        .with_initialization_script(CONTENT_INIT_SCRIPT)
        .with_ipc_handler(command_handler(proxy.clone()))
        .with_on_page_load_handler({
            let proxy = proxy.clone();
            move |event, url| {
                let _ = proxy.send_event(UserEvent::UrlChanged(url));
                let _ = proxy.send_event(UserEvent::Loading(matches!(
                    event,
                    PageLoadEvent::Started
                )));
            }
        })
        .with_document_title_changed_handler({
            let proxy = proxy.clone();
            move |title| {
                let _ = proxy.send_event(UserEvent::TitleChanged(title));
            }
        })
        .with_new_window_req_handler({
            // Open popups and target="_blank" links in the current view.
            let proxy = proxy.clone();
            move |url, _features| {
                let _ = proxy.send_event(UserEvent::Command(format!("nav:{url}")));
                NewWindowResponse::Deny
            }
        })
        .build_as_child(&window)?;

    set_toolbar_url(&toolbar, &start_url);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. },
                ..
            } => {
                let (toolbar_bounds, content_bounds) = layout(&window);
                let _ = toolbar.set_bounds(toolbar_bounds);
                let _ = content.set_bounds(content_bounds);
            }
            Event::UserEvent(UserEvent::Command(cmd)) => {
                run_command(&cmd, &toolbar, &content);
            }
            Event::UserEvent(UserEvent::UrlChanged(url)) => {
                set_toolbar_url(&toolbar, &url);
            }
            Event::UserEvent(UserEvent::Loading(on)) => {
                let _ = toolbar.evaluate_script(&format!("browsecraft.setLoading({on})"));
            }
            Event::UserEvent(UserEvent::TitleChanged(title)) => {
                let title = title.trim();
                window.set_title(&if title.is_empty() {
                    "Browsecraft".to_string()
                } else {
                    format!("{title} - Browsecraft")
                });
            }
            _ => {}
        }
    });
}

fn command_handler(proxy: EventLoopProxy<UserEvent>) -> impl Fn(wry::http::Request<String>) {
    move |req| {
        let _ = proxy.send_event(UserEvent::Command(req.into_body()));
    }
}

fn run_command(cmd: &str, toolbar: &WebView, content: &WebView) {
    let result = match cmd {
        "back" => content.evaluate_script("history.back()"),
        "forward" => content.evaluate_script("history.forward()"),
        "reload" => content.reload(),
        "stop" => content.evaluate_script("window.stop()"),
        "home" => content.load_url(HOME_URL),
        "focus-url" => {
            let _ = toolbar.focus();
            toolbar.evaluate_script("browsecraft.focusUrl()")
        }
        _ => match cmd.strip_prefix("nav:") {
            Some(input) => {
                let url = address::resolve(input);
                set_toolbar_url(toolbar, &url);
                let _ = content.focus();
                content.load_url(&url)
            }
            None => Ok(()),
        },
    };
    if let Err(err) = result {
        eprintln!("command {cmd:?} failed: {err}");
    }
}

fn set_toolbar_url(toolbar: &WebView, url: &str) {
    let arg = serde_json::to_string(url).unwrap_or_default();
    let _ = toolbar.evaluate_script(&format!("browsecraft.setUrl({arg})"));
}

/// Splits the window into the toolbar strip and the page area below it.
fn layout(window: &Window) -> (Rect, Rect) {
    let size = window.inner_size();
    let bar = (TOOLBAR_HEIGHT * window.scale_factor()).round() as u32;
    let bar = bar.min(size.height);
    let toolbar = Rect {
        position: PhysicalPosition::new(0, 0).into(),
        size: PhysicalSize::new(size.width, bar).into(),
    };
    let content = Rect {
        position: PhysicalPosition::new(0, bar as i32).into(),
        size: PhysicalSize::new(size.width, size.height - bar).into(),
    };
    (toolbar, content)
}
