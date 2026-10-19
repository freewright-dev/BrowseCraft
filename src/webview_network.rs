//! Native WebView2 network filtering, including parser-initiated resources.

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{WebResourceRequestedEventHandler, take_pwstr};
use windows::core::{Interface, PWSTR, w};
use wry::WebViewExtWindows;

use crate::adblock::{self, Decision};

pub fn install(wv: &wry::WebView) -> windows::core::Result<()> {
    unsafe {
        let view = wv.controller().CoreWebView2()?;
        install_filter(&view, &wv.environment(), adblock::instance())
    }
}

fn resource_type(kind: COREWEBVIEW2_WEB_RESOURCE_CONTEXT) -> u32 {
    match kind {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => adblock::TYPE_MAIN_FRAME,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => adblock::TYPE_STYLESHEET,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => adblock::TYPE_IMAGE,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => adblock::TYPE_SCRIPT,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => adblock::TYPE_FONT,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST
        | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH => adblock::TYPE_XHR,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => adblock::TYPE_MEDIA,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET => adblock::TYPE_WEBSOCKET,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING => adblock::TYPE_PING,
        _ => adblock::TYPE_OTHER,
    }
}

fn install_filter(
    view: &ICoreWebView2,
    env: &ICoreWebView2Environment,
    blocker: &'static adblock::AdBlocker,
) -> windows::core::Result<()> {
    unsafe {
        // Include frame and worker requests when supported by the runtime.
        if let Ok(view) = view.cast::<ICoreWebView2_22>() {
            view.AddWebResourceRequestedFilterWithRequestSourceKinds(
                w!("*"),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
            )?;
        } else {
            view.AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
        }
        let env = env.clone();
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |sender, args| {
            let Some(args) = args else {
                return Ok(());
            };
            let request = args.Request()?;
            let mut uri = PWSTR::null();
            request.Uri(&mut uri)?;
            let uri = take_pwstr(uri);
            let Ok(url) = url::Url::parse(&uri) else {
                return Ok(());
            };
            if !matches!(url.scheme(), "http" | "https") {
                return Ok(());
            }
            // From here on every request is checked: a failed lookup only
            // leaves its detail at the default rather than skipping the
            // filter.
            let mut kind = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
            let _ = args.ResourceContext(&mut kind);
            let mut type_bits = resource_type(kind);
            let mut source = String::new();
            if let Ok(headers) = request.Headers() {
                if kind == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT {
                    let mut value = PWSTR::null();
                    let _ = headers.GetHeader(w!("Sec-Fetch-Dest"), &mut value);
                    if matches!(take_pwstr(value).as_str(), "iframe" | "frame") {
                        type_bits = adblock::TYPE_SUB_FRAME;
                    }
                }
                for name in [w!("Origin"), w!("Referer")] {
                    let mut value = PWSTR::null();
                    let result = headers.GetHeader(name, &mut value);
                    let value = take_pwstr(value);
                    if result.is_ok() && url::Url::parse(&value).is_ok() {
                        source = value;
                        break;
                    }
                }
            }
            if source.is_empty() {
                if let Some(sender) = sender {
                    let mut value = PWSTR::null();
                    if sender.Source(&mut value).is_ok() {
                        source = take_pwstr(value);
                    }
                }
            }
            let source = url::Url::parse(&source).ok();
            let host = source.as_ref().and_then(|u| u.host_str()).unwrap_or("");
            if blocker.matches_network(url.as_str(), url.host_str().unwrap_or(""), type_bits, host)
                == Decision::Block
            {
                let response = env.CreateWebResourceResponse(
                    None,
                    403,
                    w!("Blocked by BrowseCraft"),
                    w!("Content-Length: 0\r\nCache-Control: no-store\r\n"),
                )?;
                args.SetResponse(&response)?;
            }
            Ok(())
        }));
        let mut token = 0;
        view.add_WebResourceRequested(&handler, &mut token)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::pump_events::EventLoopExtPumpEvents;
    use winit::platform::windows::EventLoopBuilderExtWindows;
    use winit::window::{Window, WindowId};

    #[test]
    #[ignore = "requires a Windows desktop and the WebView2 runtime"]
    fn native_requests_and_redirects_are_blocked_before_network() {
        use crate::test_http::{Server, ok, redirect};
        let server = Server::new(|request| {
            match request.path.as_str() {
            "/" => ok(r#"<!doctype html><img src='/blocked-image'><img src='/redirect-image'>
                <link rel='stylesheet' href='/blocked.css'><script src='/blocked.js'></script>
                <iframe id=f></iframe><script>
                window.onload=async()=>{
                  const u=location.origin+'/blocked-fetch';
                  const ForeignRequest=document.querySelector('iframe').contentWindow.Request;
                  const foreign=new ForeignRequest(u);
                  await Promise.allSettled([fetch(u),fetch(new URL(u)),fetch(new Request(u)),fetch(foreign)]);
                  await fetch('/done');
                };</script>"#).replace("Content-Length:", "Content-Type: text/html\r\nContent-Length:"),
            "/redirect-image" => redirect(302, "/blocked-redirect"),
            _ => ok("done"),
        }
        });
        let mut blocker = adblock::AdBlocker::empty();
        blocker.add_filter_list("test", "/blocked");
        let blocker = Box::leak(Box::new(blocker));
        struct App {
            url: String,
            blocker: &'static adblock::AdBlocker,
            window: Option<Window>,
            view: Option<wry::WebView>,
        }
        impl ApplicationHandler for App {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                if self.window.is_some() {
                    return;
                }
                let window = event_loop
                    .create_window(Window::default_attributes().with_visible(false))
                    .unwrap();
                let view = wry::WebViewBuilder::new().build_as_child(&window).unwrap();
                unsafe {
                    install_filter(
                        &view.controller().CoreWebView2().unwrap(),
                        &view.environment(),
                        self.blocker,
                    )
                    .unwrap();
                }
                view.load_url(&self.url).unwrap();
                self.view = Some(view);
                self.window = Some(window);
            }
            fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
        }
        let mut event_loop = EventLoop::builder().with_any_thread(true).build().unwrap();
        let mut app = App {
            url: server.url.to_string(),
            blocker,
            window: None,
            view: None,
        };
        let start = Instant::now();
        loop {
            event_loop.pump_app_events(Some(Duration::from_millis(20)), &mut app);
            if server
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.path == "/done")
            {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "WebView2 fixture did not finish: {:?}",
                server.requests.lock().unwrap()
            );
        }
        let requests = server.requests.lock().unwrap();
        assert!(requests.iter().any(|r| r.path == "/redirect-image"));
        assert!(
            !requests.iter().any(|r| r.path.starts_with("/blocked")),
            "{requests:?}"
        );
    }
}
