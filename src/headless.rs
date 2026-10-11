//! Renders a page to a PNG without opening a window:
//! `browsecraft --screenshot out.png <url> [width]`.

use std::collections::HashMap;
use std::rc::Rc;

use crate::font::Fonts;
use crate::layout::{self, FormState, LayoutInput};
use crate::loader::{self, Request};
use crate::paint::{self, Canvas};
use crate::style::{self, StyleContext};

const MAX_HEIGHT: f32 = 6000.0;

pub fn screenshot(fonts: &Fonts, url: &str, out: &str, width: u32) -> Result<(), String> {
    let (page, images) = loader::load_blocking(&Request::Get(url.to_string()));
    let images: HashMap<_, _> = images.into_iter().map(|(src, img)| (src, Rc::new(img))).collect();
    let (vw, vh) = (width as f32, 800.0);
    let ctx = StyleContext { zoom: 1.0, viewport_width: vw, viewport_height: vh };
    let styles = style::compute_styles(&page.doc, &page.sheets, &ctx);
    let form = FormState::default();
    let input = LayoutInput {
        doc: &page.doc,
        styles: &styles,
        fonts,
        images: &images,
        form: &form,
        zoom: 1.0,
        viewport_width: vw,
        viewport_height: vh,
    };
    let layout = layout::layout(&input);
    let height = layout.height.clamp(vh, MAX_HEIGHT).ceil() as u32;
    let mut pixels = vec![0u32; (width * height) as usize];
    let mut canvas = Canvas::new(&mut pixels, width as usize, height as usize);
    let full = layout::Rect { x: 0.0, y: 0.0, w: vw, h: height as f32 };
    canvas.fill_rect(full, layout.background);
    paint::paint_items(&mut canvas, &layout.items, fonts, &images, 0.0, 0.0);

    let rgb: Vec<u8> = pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    image::save_buffer(out, &rgb, width, height, image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())
}
