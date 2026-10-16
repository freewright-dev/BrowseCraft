//! Screen capture used by the "save a screenshot" shortcut.
//!
//! BrowseCraft's own frame only holds engine-rendered pixels; an embedded
//! WebView2 page is a separate native window, so the only way to capture "the
//! page as shown" is to grab the screen. On Windows that is a GDI `BitBlt`
//! from the desktop DC. Everywhere else the shortcut falls back to the engine
//! frame (handled by the caller).

/// Captures a rectangle of the screen and returns tightly packed RGBA
/// (`w * h * 4` bytes). Coordinates are physical screen pixels. Returns `None`
/// when the platform can't capture or the region is empty.
#[cfg(windows)]
pub fn screen_region(x: i32, y: i32, w: u32, h: u32) -> Option<Vec<u8>> {
    imp::capture(x, y, w, h)
}

#[cfg(not(windows))]
pub fn screen_region(_x: i32, _y: i32, _w: u32, _h: u32) -> Option<Vec<u8>> {
    None
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

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

    const SRCCOPY: u32 = 0x00CC_0020;
    const DIB_RGB_COLORS: u32 = 0;
    const BI_RGB: u32 = 0;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetDC(hwnd: *mut c_void) -> Hdc;
        fn ReleaseDC(hwnd: *mut c_void, hdc: Hdc) -> i32;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
        fn CreateCompatibleBitmap(hdc: Hdc, w: i32, h: i32) -> Hgdiobj;
        fn SelectObject(hdc: Hdc, obj: Hgdiobj) -> Hgdiobj;
        fn BitBlt(
            dst: Hdc,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            src: Hdc,
            sx: i32,
            sy: i32,
            rop: u32,
        ) -> i32;
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

    pub fn capture(x: i32, y: i32, w: u32, h: u32) -> Option<Vec<u8>> {
        if w == 0 || h == 0 || w > 32_768 || h > 32_768 {
            return None;
        }
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            if screen.is_null() {
                return None;
            }
            let mem = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, w as i32, h as i32);
            if mem.is_null() || bmp.is_null() {
                if !mem.is_null() {
                    DeleteDC(mem);
                }
                if !bmp.is_null() {
                    DeleteObject(bmp);
                }
                ReleaseDC(std::ptr::null_mut(), screen);
                return None;
            }

            let old = SelectObject(mem, bmp);
            let blt = BitBlt(mem, 0, 0, w as i32, h as i32, screen, x, y, SRCCOPY) != 0;
            let mut info = BitmapInfo {
                header: BitmapInfoHeader {
                    size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                    width: w as i32,
                    height: -(h as i32), // negative height: top-down rows
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
            let mut buf = vec![0u8; w as usize * h as usize * 4];
            let lines = if blt {
                GetDIBits(
                    mem,
                    bmp,
                    0,
                    h,
                    buf.as_mut_ptr() as *mut c_void,
                    &mut info,
                    DIB_RGB_COLORS,
                )
            } else {
                0
            };

            SelectObject(mem, old);
            DeleteObject(bmp);
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);

            if lines == 0 {
                return None;
            }
            for px in buf.chunks_exact_mut(4) {
                px.swap(0, 2); // BGRA -> RGBA
                px[3] = 255;
            }
            Some(buf)
        }
    }
}
