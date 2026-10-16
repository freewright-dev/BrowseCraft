//! Window capture and the Pictures folder, for the "save a screenshot"
//! shortcut.
//!
//! BrowseCraft's own frame only holds engine-rendered pixels; an embedded
//! WebView2 page is a separate native child window. On Windows the shortcut
//! asks the window to render itself (`PrintWindow` with
//! `PW_RENDERFULLCONTENT`), which includes that child but, unlike reading
//! the screen, never other windows lying on top. Everywhere else it falls
//! back to the engine frame (handled by the caller).

use std::path::PathBuf;

/// Renders a rectangle of `window`'s client area and returns tightly packed
/// RGBA (`w * h * 4` bytes). Coordinates are physical client pixels. Returns
/// `None` when the platform can't capture or the region is empty.
#[cfg(windows)]
pub fn window_region(
    window: &winit::window::Window,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
) -> Option<Vec<u8>> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    imp::capture(handle.hwnd.get() as *mut std::ffi::c_void, x, y, w, h)
}

#[cfg(not(windows))]
pub fn window_region(
    _window: &winit::window::Window,
    _x: i32,
    _y: i32,
    _w: u32,
    _h: u32,
) -> Option<Vec<u8>> {
    None
}

/// The user's Pictures folder as Windows knows it (OneDrive and folder
/// redirection included).
#[cfg(windows)]
pub fn pictures_dir() -> Option<PathBuf> {
    imp::pictures_dir()
}

#[cfg(not(windows))]
pub fn pictures_dir() -> Option<PathBuf> {
    None
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;

    type Hwnd = *mut c_void;
    type Hdc = *mut c_void;
    type Hgdiobj = *mut c_void;

    #[repr(C)]
    #[allow(dead_code)]
    struct BitmapInfoHeader {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bit_count: u16,
        compression: u32,
        size_image: u32,
        x_pels_per_meter: i32,
        y_pels_per_meter: i32,
        clr_used: u32,
        clr_important: u32,
    }

    #[repr(C)]
    #[allow(dead_code)]
    struct RgbQuad {
        blue: u8,
        green: u8,
        red: u8,
        reserved: u8,
    }

    #[repr(C)]
    struct BitmapInfo {
        header: BitmapInfoHeader,
        colors: [RgbQuad; 1],
    }

    #[repr(C)]
    #[derive(Default)]
    struct WinRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    /// {33E28130-4E1E-4676-835A-98395C3BC3BB}
    const FOLDERID_PICTURES: Guid = Guid {
        data1: 0x33E2_8130,
        data2: 0x4E1E,
        data3: 0x4676,
        data4: [0x83, 0x5A, 0x98, 0x39, 0x5C, 0x3B, 0xC3, 0xBB],
    };
    const KF_FLAG_CREATE: u32 = 0x0000_8000;

    const PW_CLIENTONLY: u32 = 0x1;
    const PW_RENDERFULLCONTENT: u32 = 0x2;
    const DIB_RGB_COLORS: u32 = 0;
    const BI_RGB: u32 = 0;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetDC(hwnd: Hwnd) -> Hdc;
        fn ReleaseDC(hwnd: Hwnd, hdc: Hdc) -> i32;
        fn GetClientRect(hwnd: Hwnd, rect: *mut WinRect) -> i32;
        fn PrintWindow(hwnd: Hwnd, hdc: Hdc, flags: u32) -> i32;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
        fn CreateCompatibleBitmap(hdc: Hdc, w: i32, h: i32) -> Hgdiobj;
        fn SelectObject(hdc: Hdc, obj: Hgdiobj) -> Hgdiobj;
        fn GetDIBits(
            hdc: Hdc,
            bmp: Hgdiobj,
            start: u32,
            lines: u32,
            bits: *mut c_void,
            info: *mut BitmapInfo,
            usage: u32,
        ) -> i32;
        fn DeleteDC(hdc: Hdc) -> i32;
        fn DeleteObject(obj: Hgdiobj) -> i32;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHGetKnownFolderPath(
            id: *const Guid,
            flags: u32,
            token: *mut c_void,
            path: *mut *mut u16,
        ) -> i32;
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(ptr: *mut c_void);
    }

    pub fn capture(hwnd: Hwnd, x: i32, y: i32, w: u32, h: u32) -> Option<Vec<u8>> {
        if w == 0 || h == 0 || x < 0 || y < 0 {
            return None;
        }
        unsafe {
            let mut client = WinRect::default();
            if GetClientRect(hwnd, &mut client) == 0 {
                return None;
            }
            let (cw, ch) = (client.right - client.left, client.bottom - client.top);
            if cw <= 0 || ch <= 0 || cw > 32_768 || ch > 32_768 {
                return None;
            }
            // The region must lie inside the client area.
            let (x1, y1) = (x.checked_add(w as i32)?, y.checked_add(h as i32)?);
            if x1 > cw || y1 > ch {
                return None;
            }

            let dc = GetDC(hwnd);
            if dc.is_null() {
                return None;
            }
            let mem = CreateCompatibleDC(dc);
            let bmp = CreateCompatibleBitmap(dc, cw, ch);
            if mem.is_null() || bmp.is_null() {
                if !mem.is_null() {
                    DeleteDC(mem);
                }
                if !bmp.is_null() {
                    DeleteObject(bmp);
                }
                ReleaseDC(hwnd, dc);
                return None;
            }

            let old = SelectObject(mem, bmp);
            let printed = PrintWindow(hwnd, mem, PW_CLIENTONLY | PW_RENDERFULLCONTENT) != 0;
            // GetDIBits requires the bitmap not to be selected into a DC.
            SelectObject(mem, old);
            let mut info = BitmapInfo {
                header: BitmapInfoHeader {
                    size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                    width: cw,
                    height: -ch, // negative height: top-down rows
                    planes: 1,
                    bit_count: 32,
                    compression: BI_RGB,
                    size_image: 0,
                    x_pels_per_meter: 0,
                    y_pels_per_meter: 0,
                    clr_used: 0,
                    clr_important: 0,
                },
                colors: [RgbQuad {
                    blue: 0,
                    green: 0,
                    red: 0,
                    reserved: 0,
                }],
            };
            let mut full = vec![0u8; cw as usize * ch as usize * 4];
            let lines = if printed {
                GetDIBits(
                    dc,
                    bmp,
                    0,
                    ch as u32,
                    full.as_mut_ptr() as *mut c_void,
                    &mut info,
                    DIB_RGB_COLORS,
                )
            } else {
                0
            };

            DeleteObject(bmp);
            DeleteDC(mem);
            ReleaseDC(hwnd, dc);

            if lines != ch {
                return None;
            }
            let (row_bytes, stride) = (w as usize * 4, cw as usize * 4);
            let mut out = Vec::with_capacity(row_bytes * h as usize);
            for row in y as usize..y1 as usize {
                let start = row * stride + x as usize * 4;
                out.extend_from_slice(&full[start..start + row_bytes]);
            }
            for px in out.chunks_exact_mut(4) {
                px.swap(0, 2); // BGRA -> RGBA
                px[3] = 255;
            }
            Some(out)
        }
    }

    pub fn pictures_dir() -> Option<PathBuf> {
        unsafe {
            let mut path: *mut u16 = std::ptr::null_mut();
            let hr = SHGetKnownFolderPath(
                &FOLDERID_PICTURES,
                KF_FLAG_CREATE,
                std::ptr::null_mut(),
                &mut path,
            );
            // The buffer must be freed even when the call fails.
            let dir = (hr >= 0 && !path.is_null()).then(|| {
                let len = (0..).take_while(|&i| *path.add(i) != 0).count();
                let wide = std::slice::from_raw_parts(path, len);
                PathBuf::from(std::ffi::OsString::from_wide(wide))
            });
            CoTaskMemFree(path as *mut c_void);
            dir.filter(|d| !d.as_os_str().is_empty())
        }
    }
}
