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

`deny.toml` ignores exactly the two unmaintained advisories above (and the six
servo-only ones listed under "Servo feature" below), by ID and with a reason
pointing here, so that `cargo deny check` can pass and run in CI
([issue #76](https://github.com/freewright-dev/BrowseCraft/issues/76)). There
is no blanket setting: any other advisory, including a new one for these same
crates, still fails the check. The glib finding is not reported by
`cargo deny` (it only flags unsound crates in the workspace itself) and stays
listed here. Review all three again when updating wry, GTK, fontdue or winit,
and remove an ignore as soon as its crate leaves the graph. Do not treat a
successful `cargo audit` exit as the absence of informational warnings.

## Servo feature (experimental, Linux)

Servo is now pinned by the lockfile to 0.7.0. This takes the upstream fix for
the boxless-ancestor `IntersectionObserver` hang without vendoring an engine
patch. The embedding migration removes `set_throttled`: 0.7's `show`/`hide`
now handle throttling. Compatibility policies and the native smoke test are
documented in [servo-compat.md](servo-compat.md#current-integration-servo-07).
The WebCrypto security restriction below is unchanged.

The optional `servo` dependency (issue #65) is resolved into `Cargo.lock` for
every build, but compiled only on Linux with `--features servo`. Resolving it
moved the shared ICU crates (`icu_*`) from 2.3 to 2.1 and `idna_adapter` from
1.2.2 to 1.2.1, which Servo pins. `cargo deny check` (default features)
is checked with all features (the CI action's default), so Servo's graph is
covered. It contains six unmaintained crates: `bincode` (RUSTSEC-2025-0141),
`unic-char-property` (RUSTSEC-2025-0081), `unic-char-range`
(RUSTSEC-2025-0075), `unic-common` (RUSTSEC-2025-0080), `unic-ucd-ident`
(RUSTSEC-2025-0100) and `unic-ucd-version` (RUSTSEC-2025-0098). `deny.toml`
ignores exactly these IDs, each marked as servo-only; they are "unmaintained"
notices, not vulnerabilities, and stay part of the evaluation of whether to
adopt Servo. Remove them if the feature is dropped or Servo moves off these
crates.

The feature also enables Servo's `webcrypto` cargo feature, because many
sites need `crypto.getRandomValues()`. That brings in the `rsa` crate, whose
RSA decryption has an unfixed timing side channel (RUSTSEC-2023-0071, the
"Marvin" attack). Pages could only reach it through `crypto.subtle`, which
BrowseCraft turns off (`dom_crypto_subtle_enabled = false` in
`servo_pane.rs`), so `deny.toml` ignores this advisory by ID with that reason.
Re-check it whenever `crypto.subtle` is enabled or the `rsa` crate is updated.
Servo also needs the release candidate `pkcs1 0.8.0-rc.4` (the version in
its own lockfile); the newer `0.8.0-rc.5` does not compile with the `rsa`
release candidate it uses.

## Policy and checks

The project is GPL-3.0-or-later. [Third-party notices](../THIRD_PARTY_NOTICES.md)
identify the uBlock Origin adaptation and dependency/asset obligations.
`deny.toml` explicitly allows the licenses needed by the current graph,
rejects unknown registry/git sources and reports duplicate versions.

CI runs `cargo deny check` (advisories, bans, licenses and sources) in
`.github/workflows/deny.yml` on every pull request, on pushes to `main` and
weekly, so an advisory published between changes is still noticed.

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
