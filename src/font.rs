//! System fonts, glyph fallback and glyph caches, on top of fontdue.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use fontdue::{Font, FontSettings, Metrics};

use crate::style::Family;

/// Absolute device-pixel limit, including CSS relative sizes and browser zoom.
pub const MAX_FONT_PX: f32 = 2048.0;
// Bound both raster dimensions and area before fontdue allocates its canvas.
// This also keeps its f32/i32 raster indices well below their precision limits.
const MAX_GLYPH_SIDE: usize = 4096;
const MAX_GLYPH_PIXELS: usize = 4 * 1024 * 1024;

fn safe_font_size(size: f32) -> bool {
    size.is_finite() && size > 0.0 && size <= MAX_FONT_PX
}

fn safe_glyph_metrics(metrics: &Metrics) -> bool {
    metrics.width <= MAX_GLYPH_SIDE
        && metrics.height <= MAX_GLYPH_SIDE
        && metrics.width.checked_mul(metrics.height).is_some_and(|area| area <= MAX_GLYPH_PIXELS)
}

fn empty_glyph() -> Rc<Glyph> {
    Rc::new(Glyph { metrics: Metrics::default(), coverage: Vec::new() })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontKey {
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
}

impl Eq for Family {}
impl std::hash::Hash for Family {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (*self as u8).hash(state);
    }
}

impl FontKey {
    pub const UI: FontKey = FontKey {
        family: Family::Sans,
        bold: false,
        italic: false,
    };

    /// Phosphor icons for the browser chrome. The font is embedded in the
    /// binary (see `crate::icons`), so this face is always available.
    pub const ICON: FontKey = FontKey {
        family: Family::Icon,
        bold: false,
        italic: false,
    };

    fn index(self) -> usize {
        (self.family as usize) * 4 + self.bold as usize * 2 + self.italic as usize
    }
}

/// Candidate files per face, in order of preference.
fn face_paths(key: FontKey) -> &'static [&'static str] {
    use Family::*;
    match (key.family, key.bold, key.italic) {
        (Sans, false, false) => &[
            "C:/Windows/Fonts/segoeui.ttf",
            "C:/Windows/Fonts/arial.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        ],
        (Sans, true, false) => &[
            "C:/Windows/Fonts/segoeuib.ttf",
            "C:/Windows/Fonts/arialbd.ttf",
            "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        ],
        (Sans, false, true) => &[
            "C:/Windows/Fonts/segoeuii.ttf",
            "C:/Windows/Fonts/ariali.ttf",
            "/System/Library/Fonts/Supplemental/Arial Italic.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-Oblique.ttf",
        ],
        (Sans, true, true) => &[
            "C:/Windows/Fonts/segoeuiz.ttf",
            "C:/Windows/Fonts/arialbi.ttf",
            "/System/Library/Fonts/Supplemental/Arial Bold Italic.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-BoldOblique.ttf",
        ],
        (Serif, false, false) => &[
            "C:/Windows/Fonts/times.ttf",
            "C:/Windows/Fonts/georgia.ttf",
            "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
            "/usr/share/fonts/TTF/DejaVuSerif.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf",
        ],
        (Serif, true, false) => &[
            "C:/Windows/Fonts/timesbd.ttf",
            "/System/Library/Fonts/Supplemental/Times New Roman Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSerif-Bold.ttf",
        ],
        (Serif, false, true) => &[
            "C:/Windows/Fonts/timesi.ttf",
            "/System/Library/Fonts/Supplemental/Times New Roman Italic.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Italic.ttf",
        ],
        (Serif, true, true) => &[
            "C:/Windows/Fonts/timesbi.ttf",
            "/System/Library/Fonts/Supplemental/Times New Roman Bold Italic.ttf",
        ],
        (Mono, false, _) => &[
            "C:/Windows/Fonts/consola.ttf",
            "C:/Windows/Fonts/cour.ttf",
            "/System/Library/Fonts/Supplemental/Courier New.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        ],
        (Mono, true, _) => &[
            "C:/Windows/Fonts/consolab.ttf",
            "C:/Windows/Fonts/courbd.ttf",
            "/System/Library/Fonts/Supplemental/Courier New Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Bold.ttf",
        ],
        // The Phosphor icon font is not in `face_paths`: it is embedded in the
        // binary, so it is installed directly by `Fonts::new`.
        (Family::Icon, _, _) => &[],
    }
}

/// Fonts tried for characters the page font lacks.
const FALLBACK_PATHS: &[&str] = &[
    "C:/Windows/Fonts/seguisym.ttf",
    "C:/Windows/Fonts/seguiemj.ttf",
    "C:/Windows/Fonts/msyh.ttc",
    "C:/Windows/Fonts/YuGothM.ttc",
    "C:/Windows/Fonts/malgun.ttf",
    "C:/Windows/Fonts/Nirmala.ttf",
    "/System/Library/Fonts/Apple Symbols.ttf",
    "/System/Library/Fonts/PingFang.ttc",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
];

fn load(path: &str) -> Option<Font> {
    load_face(std::path::Path::new(path))
}

fn load_face(path: &std::path::Path) -> Option<Font> {
    load_bytes(std::fs::read(path).ok()?)
}

fn load_bytes(data: Vec<u8>) -> Option<Font> {
    Font::from_bytes(
        data,
        FontSettings {
            collection_index: 0,
            scale: 40.0,
            load_substitutions: false,
        },
    )
    .ok()
}

pub struct Glyph {
    pub metrics: Metrics,
    pub coverage: Vec<u8>,
}

/// Identifies a loaded font: 0..16 are page faces, 16.. are fallbacks.
type FaceId = usize;
const FALLBACK_BASE: FaceId = 16;

pub struct Fonts {
    faces: [OnceCell<Option<Font>>; 16],
    fallbacks: Vec<OnceCell<Option<Font>>>,
    advances: RefCell<HashMap<(FaceId, char, u32), f32>>,
    glyphs: RefCell<HashMap<(FaceId, char, u32), Rc<Glyph>>>,
    /// Which face draws each character for a key.
    resolved: RefCell<HashMap<(usize, char), FaceId>>,
}

/// Font sizes are cached in quarter pixels.
fn size_key(size: f32) -> u32 {
    (size * 4.0).round() as u32
}

impl Fonts {
    /// Loads the default face. Fails if the system has none of the fonts we
    /// know about.
    pub fn new() -> Result<Fonts, String> {
        let fonts = Fonts {
            faces: Default::default(),
            fallbacks: FALLBACK_PATHS.iter().map(|_| OnceCell::new()).collect(),
            advances: RefCell::default(),
            glyphs: RefCell::default(),
            resolved: RefCell::default(),
        };
        if fonts.face(FontKey::UI).is_none() {
            return Err(
                "no usable system font found (looked for Segoe UI, Arial and DejaVu Sans)".into(),
            );
        }
        // The Phosphor icon font is embedded in the binary, so this always
        // succeeds; if it somehow does not, icons fall back to the UI face.
        if let Some(icon) = load_bytes(crate::icons::FONT_BYTES.to_vec()) {
            let _ = fonts.faces[FontKey::ICON.index()].set(Some(icon));
        }
        Ok(fonts)
    }

    fn face(&self, key: FontKey) -> Option<&Font> {
        self.faces[key.index()]
            .get_or_init(|| face_paths(key).iter().find_map(|p| load(p)))
            .as_ref()
    }

    fn font(&self, id: FaceId) -> &Font {
        if id >= FALLBACK_BASE {
            if let Some(Some(f)) = self.fallbacks[id - FALLBACK_BASE].get() {
                return f;
            }
        } else if let Some(Some(f)) = self.faces[id].get() {
            return f;
        }
        self.face(FontKey::UI).expect("default font")
    }

    /// The face that should draw `ch` for `key`: the styled face, then
    /// plainer variants of it, then the fallback fonts.
    fn face_for(&self, key: FontKey, ch: char) -> FaceId {
        if let Some(&id) = self.resolved.borrow().get(&(key.index(), ch)) {
            return id;
        }
        let candidates = [
            key,
            FontKey {
                italic: false,
                ..key
            },
            FontKey {
                bold: false,
                italic: false,
                ..key
            },
            FontKey::UI,
        ];
        let mut chosen = None;
        let mut first_available = None;
        for k in candidates {
            if let Some(font) = self.face(k) {
                first_available.get_or_insert(k.index());
                if ch.is_whitespace() || font.lookup_glyph_index(ch) != 0 {
                    chosen = Some(k.index());
                    break;
                }
            }
        }
        if chosen.is_none() {
            for (i, cell) in self.fallbacks.iter().enumerate() {
                if let Some(font) = cell.get_or_init(|| load(FALLBACK_PATHS[i]))
                    && font.lookup_glyph_index(ch) != 0
                {
                    chosen = Some(FALLBACK_BASE + i);
                    break;
                }
            }
        }
        let id = chosen.or(first_available).unwrap_or(FontKey::UI.index());
        self.resolved.borrow_mut().insert((key.index(), ch), id);
        id
    }

    pub fn advance(&self, key: FontKey, ch: char, size: f32) -> f32 {
        if !safe_font_size(size)
            || ch.is_control()
            || matches!(
                ch,
                '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{ad}' | '\u{fe0f}'
            )
        {
            return 0.0;
        }
        let face = self.face_for(key, ch);
        let k = (face, ch, size_key(size));
        if let Some(&a) = self.advances.borrow().get(&k) {
            return a;
        }
        let a = self.font(face).metrics(ch, size).advance_width;
        self.advances.borrow_mut().insert(k, a);
        a
    }

    pub fn measure(&self, key: FontKey, text: &str, size: f32) -> f32 {
        text.chars().map(|c| self.advance(key, c, size)).sum()
    }

    /// Ascent and descent (both positive) of the face's line metrics.
    pub fn vertical_metrics(&self, key: FontKey, size: f32) -> (f32, f32) {
        if !safe_font_size(size) {
            return (0.0, 0.0);
        }
        let font = self
            .face(key)
            .or_else(|| self.face(FontKey::UI))
            .expect("default font");
        match font.horizontal_line_metrics(size) {
            Some(m) => (m.ascent, -m.descent),
            None => (size * 0.8, size * 0.2),
        }
    }

    pub fn glyph(&self, key: FontKey, ch: char, size: f32) -> Rc<Glyph> {
        // Guard direct callers as well as styled text, before any fontdue call
        // or cache lookup (NaN/overflowing sizes can alias valid cache keys).
        if !safe_font_size(size) {
            return empty_glyph();
        }
        let face = self.face_for(key, ch);
        let k = (face, ch, size_key(size));
        if let Some(g) = self.glyphs.borrow().get(&k) {
            return g.clone();
        }
        let font = self.font(face);
        if !safe_glyph_metrics(&font.metrics(ch, size)) {
            return empty_glyph();
        }
        let (metrics, coverage) = font.rasterize(ch, size);
        let glyph = Rc::new(Glyph { metrics, coverage });
        let mut cache = self.glyphs.borrow_mut();
        // Keep memory bounded on pages that use many sizes.
        if cache.len() > 20_000 {
            cache.clear();
        }
        cache.insert(k, glyph.clone());
        glyph
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_font_sizes_never_reach_fontdue() {
        let fonts = Fonts::new().expect("system fonts");
        for size in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY,
            MAX_FONT_PX + 1.0, 20_000.0, 50_000.0, 1e6, 3e9, 1e30, f32::MAX]
        {
            let glyph = fonts.glyph(FontKey::UI, 'W', size);
            assert!(glyph.coverage.is_empty(), "size {size}");
            assert_eq!((glyph.metrics.width, glyph.metrics.height), (0, 0));
            assert_eq!(fonts.advance(FontKey::UI, 'W', size), 0.0);
            assert_eq!(fonts.vertical_metrics(FontKey::UI, size), (0.0, 0.0));
        }
        assert!(fonts.glyphs.borrow().is_empty());
        assert!(fonts.advances.borrow().is_empty());
    }

    #[test]
    fn raster_budget_checks_dimensions_and_area() {
        let metrics = |width, height| Metrics { width, height, ..Metrics::default() };
        assert!(safe_glyph_metrics(&metrics(2048, 2048)));
        assert!(!safe_glyph_metrics(&metrics(2049, 2048)));
        assert!(!safe_glyph_metrics(&metrics(MAX_GLYPH_SIDE + 1, 1)));
        assert!(!safe_glyph_metrics(&metrics(1, MAX_GLYPH_SIDE + 1)));
        assert!(!safe_glyph_metrics(&metrics(usize::MAX, usize::MAX)));
    }

    #[test]
    fn supported_font_sizes_still_render() {
        let fonts = Fonts::new().expect("system fonts");
        for size in [16.0, MAX_FONT_PX] {
            let glyph = fonts.glyph(FontKey::UI, 'W', size);
            assert!(!glyph.coverage.is_empty());
            assert!(safe_glyph_metrics(&glyph.metrics));
            assert_eq!(glyph.coverage.len(), glyph.metrics.width * glyph.metrics.height);
            assert!(fonts.advance(FontKey::UI, 'W', size) > 0.0);
        }
    }

    /// Phosphor glyphs the chrome uses. If any of these move or vanish in a
    /// font update, the toolbar/tab icons silently fall back to the UI font.
    const ICONS: &[(&str, &str)] = &[
        (crate::icons::BACK, "arrow-left"),
        (crate::icons::FORWARD, "arrow-right"),
        (crate::icons::RELOAD, "arrow-clockwise"),
        (crate::icons::CLOSE, "x"),
        (crate::icons::HOME, "house"),
        (crate::icons::PLUS, "plus"),
        (crate::icons::SUN, "sun"),
        (crate::icons::MOON, "moon"),
        (crate::icons::GLOBE, "globe"),
        (crate::icons::LOCK, "lock"),
        (crate::icons::GEAR, "gear"),
    ];

    #[test]
    fn icon_glyphs_present() {
        let font = load_bytes(crate::icons::FONT_BYTES.to_vec()).expect("embedded Phosphor font");
        for &(icon, name) in ICONS {
            let ch = icon.chars().next().expect("non-empty icon");
            assert!(
                font.lookup_glyph_index(ch) != 0,
                "Phosphor is missing U+{:04X} ({name})",
                ch as u32
            );
        }
    }

    #[test]
    fn icon_face_index_within_faces() {
        assert!(Family::Icon as usize * 4 < FALLBACK_BASE);
        assert_eq!(FontKey::UI.index(), 0);
    }
}
