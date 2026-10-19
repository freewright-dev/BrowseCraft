//! Experimental Servo web view (issue #65).
//!
//! On Linux with the `servo` cargo feature, web pages render with Servo
//! instead of BrowseCraft's own engine (Linux has no system web view for
//! wry to use). Servo renders offscreen into a `SoftwareRenderingContext`;
//! the browser copies each new frame into the page area and forwards mouse,
//! wheel and keyboard input. Everywhere else `ServoPane` is an empty type
//! that can never be created, so the rest of the browser needs no `cfg`.

use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use winit::keyboard::ModifiersState;

use crate::paint::Image;

#[cfg(all(feature = "servo", target_os = "linux"))]
pub use imp::ServoPane;
#[cfg(not(all(feature = "servo", target_os = "linux")))]
pub use stub::ServoPane;

/// The calls the browser makes on a web view, whichever engine draws it.
/// Coordinates are physical pixels relative to the page area.
#[allow(dead_code, reason = "the stub implements the same surface as Servo")]
pub trait PaneInput {
    fn mouse_move(&self, x: f32, y: f32);
    fn mouse_button(&self, x: f32, y: f32, button: MouseButton, state: ElementState);
    fn wheel(&self, x: f32, y: f32, delta: MouseScrollDelta);
    fn key(&self, event: &KeyEvent, modifiers: ModifiersState);
}

#[cfg(not(all(feature = "servo", target_os = "linux")))]
mod stub {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
    use winit::event_loop::EventLoopProxy;
    use winit::keyboard::ModifiersState;

    use super::{Image, PaneInput};
    use crate::browser::UserMessage;

    /// Never constructed: Servo is only built on Linux with `--features servo`.
    pub enum ServoPane {}

    #[allow(dead_code, reason = "the browser calls these only on Servo builds")]
    impl ServoPane {
        pub fn start(
            _proxy: EventLoopProxy<UserMessage>,
            _epoch: Arc<AtomicU64>,
            _width: u32,
            _height: u32,
        ) -> Option<Self> {
            None
        }
        pub fn spin(&self) {
            match *self {}
        }
        pub fn load(&self, _url: &str) -> Result<(), String> {
            match *self {}
        }
        pub fn reload(&self) {
            match *self {}
        }
        pub fn url(&self) -> String {
            match *self {}
        }
        pub fn set_zoom(&self, _zoom: f32) {
            match *self {}
        }
        pub fn resize(&self, _width: u32, _height: u32) {
            match *self {}
        }
        pub fn set_visible(&self, _visible: bool) {
            match *self {}
        }
        pub fn set_dark(&self, _dark: bool) {
            match *self {}
        }
        pub fn evaluate_script(&self, _script: &str) {
            match *self {}
        }
        pub fn take_frame(&self) -> Option<Image> {
            match *self {}
        }
    }

    impl PaneInput for ServoPane {
        fn mouse_move(&self, _x: f32, _y: f32) {
            match *self {}
        }
        fn mouse_button(&self, _x: f32, _y: f32, _button: MouseButton, _state: ElementState) {
            match *self {}
        }
        fn wheel(&self, _x: f32, _y: f32, _delta: MouseScrollDelta) {
            match *self {}
        }
        fn key(&self, _event: &KeyEvent, _modifiers: ModifiersState) {
            match *self {}
        }
    }
}

#[cfg(all(feature = "servo", target_os = "linux"))]
mod imp {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use servo::{
        Code, CreateNewWebViewRequest, DeviceIntRect, DeviceIntSize, DevicePoint, EventLoopWaker,
        InputEvent, Key, KeyState, KeyboardEvent, LoadStatus, Location, Modifiers,
        MouseButton as ServoButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent,
        NamedKey as ServoNamedKey, NavigationRequest, RenderingContext, Servo, ServoBuilder,
        SoftwareRenderingContext, Theme, WebResourceLoad, WebResourceResponse, WebView,
        WebViewBuilder, WebViewDelegate, WheelDelta, WheelEvent, WheelMode,
    };
    use url::Url;
    use winit::dpi::PhysicalSize;
    use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
    use winit::event_loop::EventLoopProxy;
    use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey};

    use super::{Image, PaneInput};
    use crate::adblock;
    use crate::browser::{UserMessage, WebNavigation, web_navigation};

    /// A Servo instance with one web view, shared by every tab (as the
    /// WebView2 pane is).
    pub struct ServoPane {
        servo: Servo,
        context: Rc<SoftwareRenderingContext>,
        view: WebView,
        delegate: Rc<Delegate>,
    }

    /// Wakes the browser's event loop when Servo has work to do; Servo may
    /// call it from any thread.
    struct Waker(Mutex<EventLoopProxy<UserMessage>>);

    impl EventLoopWaker for Waker {
        fn clone_box(&self) -> Box<dyn EventLoopWaker> {
            let proxy = self.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
            Box::new(Waker(Mutex::new(proxy)))
        }

        fn wake(&self) {
            let _ = self.0.lock().unwrap_or_else(|e| e.into_inner()).send_event(UserMessage::ServoWake);
        }
    }

    /// Turns Servo's notifications into the same `UserMessage`s the wry web
    /// view sends, so history, titles and loading state need no new code,
    /// and applies the browser's navigation and ad-filter policy.
    struct Delegate {
        proxy: EventLoopProxy<UserMessage>,
        /// The browser's web view epoch, so its stale-event check applies.
        epoch: Arc<AtomicU64>,
        /// A frame was painted since the browser last took one.
        frame_ready: Cell<bool>,
    }

    impl Delegate {
        fn send(&self, message: UserMessage) {
            let _ = self.proxy.send_event(message);
        }
    }

    /// For evaluating the slice: with `BROWSECRAFT_SERVO_TRACE` set, load
    /// progress, titles and crashes are printed with the time since start.
    fn trace(what: &str) {
        use std::sync::OnceLock;
        use std::time::Instant;
        static START: OnceLock<Option<Instant>> = OnceLock::new();
        let start = START.get_or_init(|| std::env::var_os("BROWSECRAFT_SERVO_TRACE").map(|_| Instant::now()));
        if let Some(start) = start {
            eprintln!("servo-trace {:.2}s {what}", start.elapsed().as_secs_f32());
        }
    }

    impl WebViewDelegate for Delegate {
        fn notify_new_frame_ready(&self, _webview: WebView) {
            self.frame_ready.set(true);
            self.send(UserMessage::ServoFrame);
        }

        fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
            let url = webview.url().map(String::from).unwrap_or_default();
            trace(&format!("load {status:?} {url}"));
            match status {
                // The new document exists from here on, like WebView2's
                // ContentLoading, which `WebStart` stands for.
                LoadStatus::HeadParsed => self.send(UserMessage::WebStart(url)),
                LoadStatus::Complete => self.send(UserMessage::WebLoad(url)),
                LoadStatus::Started => {}
            }
        }

        fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
            trace(&format!("title {}", title.as_deref().unwrap_or("")));
            self.send(UserMessage::WebTitle(title.unwrap_or_default()));
        }

        fn request_navigation(&self, _webview: WebView, request: NavigationRequest) {
            match web_navigation(request.url.as_str()) {
                WebNavigation::Allow => request.allow(),
                WebNavigation::Refuse => request.deny(),
                WebNavigation::External(url) => {
                    request.deny();
                    self.send(UserMessage::WebExternal(url));
                }
                WebNavigation::Open(url) => {
                    request.deny();
                    let epoch = self.epoch.load(Ordering::SeqCst);
                    self.send(UserMessage::WebOpen { epoch, url, popup: false });
                }
            }
        }

        /// Pop-ups are refused: Servo does not say whether the user opened
        /// one, so none is followed (the request is dropped unanswered).
        fn request_create_new(&self, _parent: WebView, _request: CreateNewWebViewRequest) {}

        /// Subresources the ad filter blocks fail as network errors;
        /// main-frame loads were already decided in `request_navigation`.
        fn load_web_resource(&self, webview: WebView, load: WebResourceLoad) {
            let request = load.request();
            if request.is_for_main_frame {
                return;
            }
            let page = webview.url();
            let host = page.as_ref().and_then(Url::host_str).unwrap_or("");
            let kind = resource_type(&format!("{:?}", request.destination));
            if adblock::blocked_subresource(request.url.as_str(), kind, host) {
                let url = request.url.clone();
                load.intercept(WebResourceResponse::new(url)).cancel();
            }
        }

        fn notify_crashed(&self, _webview: WebView, reason: String, _backtrace: Option<String>) {
            eprintln!("browsecraft: a Servo page crashed: {reason}");
            trace("crashed");
            self.send(UserMessage::ServoCrashed(reason));
        }
    }

    /// The ad filter's resource type for Servo's request destination, by
    /// name (its type is not re-exported by the `servo` crate).
    fn resource_type(destination: &str) -> u32 {
        match destination {
            "Document" => adblock::TYPE_MAIN_FRAME,
            "Iframe" | "Frame" | "Embed" | "Object" => adblock::TYPE_SUB_FRAME,
            "Script" | "Worker" | "SharedWorker" | "ServiceWorker" | "AudioWorklet" | "PaintWorklet" => {
                adblock::TYPE_SCRIPT
            }
            "Image" => adblock::TYPE_IMAGE,
            "Style" | "Xslt" => adblock::TYPE_STYLESHEET,
            "Font" => adblock::TYPE_FONT,
            "Audio" | "Video" | "Track" => adblock::TYPE_MEDIA,
            "Empty" => adblock::TYPE_XHR,
            _ => adblock::TYPE_OTHER,
        }
    }

    impl ServoPane {
        /// Starts Servo with one hidden web view of `width`×`height` pixels.
        /// `None` when no rendering context can be made (no OpenGL).
        pub fn start(
            proxy: EventLoopProxy<UserMessage>,
            epoch: Arc<AtomicU64>,
            width: u32,
            height: u32,
        ) -> Option<Self> {
            // rustls is built with two crypto providers here (ring for ureq,
            // aws-lc-rs for Servo), so one must be chosen for the process.
            let _ = rustls::crypto::ring::default_provider().install_default();
            let size = PhysicalSize::new(width.max(1), height.max(1));
            let context = match SoftwareRenderingContext::new(size) {
                Ok(context) => Rc::new(context),
                Err(error) => {
                    eprintln!("browsecraft: Servo could not create a rendering context ({error:?})");
                    return None;
                }
            };
            context.make_current().ok()?;
            trace("servo started");
            let servo = ServoBuilder::default()
                .event_loop_waker(Box::new(Waker(Mutex::new(proxy.clone()))))
                .build();
            let delegate = Rc::new(Delegate { proxy, epoch, frame_ready: Cell::new(false) });
            let view = WebViewBuilder::new(&servo, context.clone())
                .delegate(delegate.clone())
                .build();
            view.hide();
            Some(ServoPane { servo, context, view, delegate })
        }

        /// Lets Servo process what it woke the event loop for.
        pub fn spin(&self) {
            self.servo.spin_event_loop();
        }

        pub fn load(&self, url: &str) -> Result<(), String> {
            let url = Url::parse(url).map_err(|e| e.to_string())?;
            self.view.load(url);
            Ok(())
        }

        pub fn reload(&self) {
            self.view.reload();
        }

        pub fn url(&self) -> String {
            self.view.url().map(String::from).unwrap_or_default()
        }

        pub fn set_zoom(&self, zoom: f32) {
            self.view.set_page_zoom(zoom);
        }

        pub fn resize(&self, width: u32, height: u32) {
            let size = PhysicalSize::new(width.max(1), height.max(1));
            if self.context.size() != size {
                self.view.resize(size);
            }
        }

        /// Hidden, a page is throttled (no animation frames, no painting).
        pub fn set_visible(&self, visible: bool) {
            if visible {
                self.view.show();
            } else {
                self.view.hide();
            }
            self.view.set_throttled(!visible);
        }

        pub fn set_dark(&self, dark: bool) {
            self.view.notify_theme_change(if dark { Theme::Dark } else { Theme::Light });
        }

        /// Runs a script in the page (the password manager's fill script).
        pub fn evaluate_script(&self, script: &str) {
            self.view.evaluate_javascript(script, |_| {});
        }

        /// The newest frame, if Servo painted one since the last call:
        /// painted into the offscreen context and read back.
        pub fn take_frame(&self) -> Option<Image> {
            if !self.delegate.frame_ready.replace(false) {
                return None;
            }
            self.view.paint();
            let size = self.context.size();
            let rect = DeviceIntRect::from_size(DeviceIntSize::new(size.width as i32, size.height as i32));
            let rgba = self.context.read_to_image(rect)?;
            // For evaluating the slice: keep the newest frame as a PNG, so
            // pages can be checked without looking at the screen.
            if let Some(path) = std::env::var_os("BROWSECRAFT_SERVO_FRAME") {
                // Written aside and renamed, so a reader never sees half a file.
                let path = std::path::PathBuf::from(path);
                let partial = path.with_extension("partial.png");
                if rgba.save(&partial).is_ok() {
                    let _ = std::fs::rename(&partial, &path);
                }
            }
            let pixels = rgba
                .pixels()
                .map(|p| (p[3] as u32) << 24 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32)
                .collect();
            Some(Image { width: rgba.width(), height: rgba.height(), pixels })
        }
    }

    impl PaneInput for ServoPane {
        fn mouse_move(&self, x: f32, y: f32) {
            let event = MouseMoveEvent::new(DevicePoint::new(x, y).into());
            self.view.notify_input_event(InputEvent::MouseMove(event));
        }

        fn mouse_button(&self, x: f32, y: f32, button: MouseButton, state: ElementState) {
            let button = match button {
                MouseButton::Left => ServoButton::Primary,
                MouseButton::Middle => ServoButton::Auxiliary,
                MouseButton::Right => ServoButton::Secondary,
                MouseButton::Back => ServoButton::Back,
                MouseButton::Forward => ServoButton::Forward,
                MouseButton::Other(n) => ServoButton::Other(n),
            };
            let action = match state {
                ElementState::Pressed => MouseButtonAction::Down,
                ElementState::Released => MouseButtonAction::Up,
            };
            let event = MouseButtonEvent::new(action, button, DevicePoint::new(x, y).into());
            self.view.notify_input_event(InputEvent::MouseButton(event));
        }

        fn wheel(&self, x: f32, y: f32, delta: MouseScrollDelta) {
            let (dx, dy, mode) = match delta {
                MouseScrollDelta::LineDelta(dx, dy) => ((dx * 76.0) as f64, (dy * 76.0) as f64, WheelMode::DeltaLine),
                MouseScrollDelta::PixelDelta(d) => (d.x, d.y, WheelMode::DeltaPixel),
            };
            let delta = WheelDelta { x: dx, y: dy, z: 0.0, mode };
            let event = WheelEvent::new(delta, DevicePoint::new(x, y).into());
            self.view.notify_input_event(InputEvent::Wheel(event));
        }

        fn key(&self, event: &KeyEvent, modifiers: ModifiersState) {
            let Some(key) = servo_key(&event.logical_key) else {
                return;
            };
            let state = match event.state {
                ElementState::Pressed => KeyState::Down,
                ElementState::Released => KeyState::Up,
            };
            let mut mods = Modifiers::empty();
            mods.set(Modifiers::SHIFT, modifiers.shift_key());
            mods.set(Modifiers::CONTROL, modifiers.control_key());
            mods.set(Modifiers::ALT, modifiers.alt_key());
            mods.set(Modifiers::META, modifiers.super_key());
            let event = KeyboardEvent::new_without_event(
                state,
                key,
                Code::Unidentified,
                Location::Standard,
                mods,
                event.repeat,
                false,
            );
            self.view.notify_input_event(InputEvent::Keyboard(event));
        }
    }

    /// The keys a page sees: characters, and the editing and navigation
    /// keys. Others (function keys, media keys) stay with the browser.
    fn servo_key(key: &WinitKey) -> Option<Key> {
        Some(match key {
            WinitKey::Character(c) => Key::Character(c.to_string()),
            WinitKey::Named(NamedKey::Space) => Key::Character(" ".into()),
            WinitKey::Named(named) => Key::Named(match named {
                NamedKey::Enter => ServoNamedKey::Enter,
                NamedKey::Tab => ServoNamedKey::Tab,
                NamedKey::Backspace => ServoNamedKey::Backspace,
                NamedKey::Delete => ServoNamedKey::Delete,
                NamedKey::Escape => ServoNamedKey::Escape,
                NamedKey::ArrowLeft => ServoNamedKey::ArrowLeft,
                NamedKey::ArrowRight => ServoNamedKey::ArrowRight,
                NamedKey::ArrowUp => ServoNamedKey::ArrowUp,
                NamedKey::ArrowDown => ServoNamedKey::ArrowDown,
                NamedKey::Home => ServoNamedKey::Home,
                NamedKey::End => ServoNamedKey::End,
                NamedKey::PageUp => ServoNamedKey::PageUp,
                NamedKey::PageDown => ServoNamedKey::PageDown,
                NamedKey::Shift => ServoNamedKey::Shift,
                NamedKey::Control => ServoNamedKey::Control,
                NamedKey::Alt => ServoNamedKey::Alt,
                _ => return None,
            }),
            _ => return None,
        })
    }
}
