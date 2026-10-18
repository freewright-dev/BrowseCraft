# Dependency maintenance

Reviewed 2026-10-18 for [issue #54](https://github.com/freewright-dev/BrowseCraft/issues/54).

## Updated dependencies

- `rustls` 0.23.45 → 0.23.46, regenerated through Cargo. This fixes
  RUSTSEC-2026-0346, -0347, -0348 and -0349. The configured ureq stack uses
  rustls logging/ring/std/tls12 features; Brotli certificate decompression and
  ECH are not configured, and no custom session-store Debug logging was found.
  Advisory presence alone did not establish application-level exploitation.
- `wry` 0.53.5 → 0.55.1 replaces its old Android HTML dependency family.
  The resolved graph removes `fxhash` 0.2.1 and `rand` 0.7.3, eliminating
  RUSTSEC-2025-0057 and RUSTSEC-2026-0097 rather than suppressing them.

## Remaining tracked advisories

- **RUSTSEC-2024-0370 — proc-macro-error 1.0.4:** unmaintained build-time
  dependency through GTK/glib macros on Linux/BSD. Follow the wry/GTK upstream
  dependency migration; updating this package independently cannot replace
  the API used by its parent crates.
- **RUSTSEC-2026-0192 — ttf-parser 0.25.1:** unmaintained, through fontdue and
  platform decoration fonts. First-party font inputs are system/embedded
  fonts, not downloaded website fonts. Track a maintained fontdue backend or
  a tested migration to a maintained font parser/rasterizer. fontdue 0.9.4
  still uses this parser.
- **RUSTSEC-2024-0429 — glib 0.18.5:** unsound `VariantStrIter` in the
  Linux/BSD GTK family. First-party code does not call that iterator;
  transitive reachability is not established. The Windows native dependency
  path does not use it. Track the GTK/wry family upgrade to a corrected glib
  rather than applying an incompatible standalone version override.

These findings remain visible: `deny.toml` has no advisory ignores. Review
them again when updating wry, GTK, fontdue or winit. Do not treat a successful
`cargo audit` exit as the absence of informational warnings.

## Policy and checks

The project is GPL-3.0-or-later. [Third-party notices](../THIRD_PARTY_NOTICES.md)
identify the uBlock Origin adaptation and dependency/asset obligations.
`deny.toml` explicitly allows the licenses needed by the current graph,
rejects unknown registry/git sources and reports duplicate versions.

```sh
cargo audit
cargo deny check
cargo tree --locked --target all -i glib
cargo tree --locked --target all -i ttf-parser
cargo tree --locked -i rustls -e features
```

Validation after the update:

- Windows `cargo test --locked`: 109 tests passed; `cargo build --locked`
  validates the normal application build.
- `cargo audit`: zero vulnerability entries; the three findings above remain
  as informational warnings.
- `cargo deny check`: licenses, sources and bans pass; advisories fail for
  the remaining unmaintained dependencies. No global suppression was added.
- Full Linux testing was attempted in WSL but the environment lacks the
  required system GLib/GTK development libraries. Run the full suite on a
  suitably provisioned Linux host before claiming Linux validation.
