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

#[cfg(any(windows, test))]
const MAX_CAPTURE_BYTES: usize = 128 * 1024 * 1024;

#[cfg(any(windows, test))]
struct CaptureLayout {
    full_bytes: usize,
    output_bytes: usize,
    stride: usize,
    row_bytes: usize,
    start: usize,
}

#[cfg(any(windows, test))]
impl CaptureLayout {
    fn new(cw: i32, ch: i32, x: i32, y: i32, w: u32, h: u32) -> Option<Self> {
        if cw <= 0 || ch <= 0 || cw > 32_768 || ch > 32_768 || x < 0 || y < 0 || w == 0 || h == 0 {
            return None;
        }
        let (wi, hi) = (i32::try_from(w).ok()?, i32::try_from(h).ok()?);
        if x.checked_add(wi)? > cw || y.checked_add(hi)? > ch {
            return None;
        }
        let stride = usize::try_from(cw).ok()?.checked_mul(4)?;
        let full_bytes = stride.checked_mul(usize::try_from(ch).ok()?)?;
        let row_bytes = usize::try_from(w).ok()?.checked_mul(4)?;
        let output_bytes = row_bytes.checked_mul(usize::try_from(h).ok()?)?;
        if full_bytes > MAX_CAPTURE_BYTES || output_bytes > MAX_CAPTURE_BYTES {
            return None;
        }
        let start = usize::try_from(y)
            .ok()?
            .checked_mul(stride)?
            .checked_add(usize::try_from(x).ok()?.checked_mul(4)?)?;
        let end = start
            .checked_add((h as usize - 1).checked_mul(stride)?)?
            .checked_add(row_bytes)?;
        if end > full_bytes {
            return None;
        }
        Some(Self {
            full_bytes,
            output_bytes,
            stride,
            row_bytes,
            start,
        })
    }
}

#[cfg(windows)]
fn zeroed_buffer(bytes: usize) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(bytes).ok()?;
    buffer.resize(bytes, 0);
    Some(buffer)
}

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

    struct CaptureResources {
        hwnd: Hwnd,
        dc: Hdc,
        mem: Hdc,
        bmp: Hgdiobj,
    }

    impl Drop for CaptureResources {
        fn drop(&mut self) {
            unsafe {
                // Destroying the memory DC first also releases a selected
                // bitmap when restoration failed on an error path.
                if !self.mem.is_null() {
                    DeleteDC(self.mem);
                }
                if !self.bmp.is_null() {
                    DeleteObject(self.bmp);
                }
                if !self.dc.is_null() {
                    ReleaseDC(self.hwnd, self.dc);
                }
            }
        }
    }

    fn valid_object(object: Hgdiobj) -> bool {
        !object.is_null() && object as isize != -1
    }

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
            let (cw, ch) = (
                client.right.checked_sub(client.left)?,
                client.bottom.checked_sub(client.top)?,
            );
            let layout = super::CaptureLayout::new(cw, ch, x, y, w, h)?;
            // Both fallible allocations happen before any GDI handles exist.
            let mut full = super::zeroed_buffer(layout.full_bytes)?;
            let mut out = super::zeroed_buffer(layout.output_bytes)?;

            let dc = GetDC(hwnd);
            if dc.is_null() {
                return None;
            }
            let mut resources = CaptureResources {
                hwnd,
                dc,
                mem: std::ptr::null_mut(),
                bmp: std::ptr::null_mut(),
            };
            resources.mem = CreateCompatibleDC(dc);
            resources.bmp = CreateCompatibleBitmap(dc, cw, ch);
            let (mem, bmp) = (resources.mem, resources.bmp);
            if mem.is_null() || bmp.is_null() {
                return None;
            }

            let old = SelectObject(mem, bmp);
            if !valid_object(old) {
                return None;
            }
            let printed = PrintWindow(hwnd, mem, PW_CLIENTONLY | PW_RENDERFULLCONTENT) != 0;
            // GetDIBits requires the bitmap not to be selected into a DC.
            let restored = SelectObject(mem, old);
            if !valid_object(restored) || restored != bmp {
                return None;
            }
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

            drop(resources);

            if lines != ch {
                return None;
            }
            for (row, dest) in out.chunks_exact_mut(layout.row_bytes).enumerate() {
                let start = layout.start + row * layout.stride;
                dest.copy_from_slice(&full[start..start + layout.row_bytes]);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn native_capture_accepts_normal_crops_and_rejects_extremes() {
        use std::ffi::c_void;
        #[link(name = "user32")]
        unsafe extern "system" {
            fn CreateWindowExW(
                ex: u32,
                class: *const u16,
                name: *const u16,
                style: u32,
                x: i32,
                y: i32,
                w: i32,
                h: i32,
                parent: *mut c_void,
                menu: *mut c_void,
                instance: *mut c_void,
                param: *mut c_void,
            ) -> *mut c_void;
            fn DestroyWindow(hwnd: *mut c_void) -> i32;
        }
        struct Window(*mut c_void);
        impl Drop for Window {
            fn drop(&mut self) {
                unsafe {
                    DestroyWindow(self.0);
                }
            }
        }
        let class: Vec<_> = "STATIC\0".encode_utf16().collect();
        let null = std::ptr::null_mut();
        let window = Window(unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                0x9000_0000,
                0,
                0,
                64,
                64,
                null,
                null,
                null,
                null,
            )
        });
        assert!(!window.0.is_null());
        let crop = imp::capture(window.0, 4, 5, 16, 12).expect("capture of native STATIC window");
        assert_eq!(crop.len(), 16 * 12 * 4);
        assert!(crop.chunks_exact(4).all(|p| p[3] == 255));
        for (w, h) in [(u32::MAX, 1), (1, u32::MAX), (32_768, 32_768), (65, 64)] {
            assert!(imp::capture(window.0, 0, 0, w, h).is_none());
        }
        assert!(imp::capture(null, 0, 0, 1, 1).is_none());
    }

    #[test]
    fn capture_layout_rejects_narrowing_overflow_and_excess_allocations() {
        for (cw, ch, x, y, w, h) in [
            (100, 100, 0, 0, u32::MAX, 1),
            (100, 100, 0, 0, 1, u32::MAX),
            (32_768, 32_768, 0, 0, 1, 1),
            (0, 100, 0, 0, 1, 1),
            (100, 100, -1, 0, 1, 1),
            (100, 100, 0, -1, 1, 1),
            (100, 100, 99, 99, 2, 1),
            (100, 100, 99, 99, 1, 2),
            (100, 100, i32::MAX, 0, 1, 1),
            (100, 100, 0, 0, 0, 1),
        ] {
            assert!(CaptureLayout::new(cw, ch, x, y, w, h).is_none());
        }
        let layout = CaptureLayout::new(100, 80, 90, 70, 10, 10).unwrap();
        assert_eq!(layout.full_bytes, 32_000);
        assert_eq!(layout.output_bytes, 400);
        assert_eq!(
            layout.start + 9 * layout.stride + layout.row_bytes,
            layout.full_bytes
        );
        assert!(CaptureLayout::new(8192, 4096, 0, 0, 8192, 4096).is_some());
        assert!(CaptureLayout::new(8192, 4097, 0, 0, 1, 1).is_none());
    }
}
