//! Gives the main thread an 8 MiB stack on Windows, where the default is
//! 1 MiB (Linux and macOS already default to 8 MiB). The fallback engine
//! styles, lays out and paints on the main thread, recursing once per DOM
//! level; at `html::MAX_DEPTH` that needs about 2 MiB in release builds and
//! 4 MiB in debug builds.

const MAIN_STACK: u32 = 8 << 20;

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "windows" {
        if env == "msvc" {
            println!("cargo:rustc-link-arg-bins=/STACK:{MAIN_STACK}");
        } else {
            println!("cargo:rustc-link-arg-bins=-Wl,--stack,{MAIN_STACK}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
