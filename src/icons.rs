//! Browser-chrome icons, from the [Phosphor] icon set via `egui-phosphor`.
//!
//! The Phosphor font is embedded in the binary by the crate, so icons render
//! everywhere regardless of the working directory (unlike the old Font Awesome
//! `assets/fa-solid-900.ttf`, which had to sit next to the executable).
//!
//! Both front ends use this module: the egui chrome installs the font through
//! [`add_fonts`] and draws the constants below, while the engine chrome loads
//! [`FONT_BYTES`] into its `FontKey::ICON` face.
//!
//! [Phosphor]: https://phosphoricons.com/

/// BrowseCraft's logo, shared by the window icon and built-in new-tab page.
pub const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");

/// egui font family the Phosphor face is registered under.
pub const FAMILY: &str = "phosphor";

/// The regular-weight Phosphor font, embedded in the binary.
pub const FONT_BYTES: &[u8] = egui_phosphor::bytes::regular::FONT;

// Codepoints for the icons the chrome draws. See the Phosphor catalogue.
pub const BACK: &str = egui_phosphor::regular::ARROW_LEFT;
pub const FORWARD: &str = egui_phosphor::regular::ARROW_RIGHT;
pub const RELOAD: &str = egui_phosphor::regular::ARROW_CLOCKWISE;
pub const CLOSE: &str = egui_phosphor::regular::X;
pub const HOME: &str = egui_phosphor::regular::HOUSE;
pub const PLUS: &str = egui_phosphor::regular::PLUS;
pub const SUN: &str = egui_phosphor::regular::SUN;
pub const MOON: &str = egui_phosphor::regular::MOON;
pub const GLOBE: &str = egui_phosphor::regular::GLOBE;
pub const LOCK: &str = egui_phosphor::regular::LOCK;
pub const GEAR: &str = egui_phosphor::regular::GEAR;

/// The Phosphor family, for `RichText::family`.
pub fn family() -> egui::FontFamily {
    egui::FontFamily::Name(FAMILY.into())
}

/// Adds the Phosphor face to `fonts` and to the proportional fallback list so
/// icon glyphs can be mixed into ordinary labels.
pub fn add_fonts(fonts: &mut egui::FontDefinitions) {
    egui_phosphor::add_to_fonts(fonts, egui_phosphor::Variant::Regular);
}
