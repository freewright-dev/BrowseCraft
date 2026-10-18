# Third-party notices

## uBlock Origin

BrowseCraft's `src/adblock.rs` identifies adaptations of uBlock Origin's
`src/js/static-net-filtering.js`, `static-filtering-parser.js`,
`cosmetic-filtering.js`, and content-script behavior.

uBlock Origin — a comprehensive, efficient content blocker

Copyright (C) 2014-present Raymond Hill and the respective contributors.

Upstream: <https://github.com/gorhill/uBlock>

These upstream files are licensed under the GNU General Public License,
version 3 or (at your option) any later version. BrowseCraft is distributed
under the same GPL-3.0-or-later terms. The complete GPLv3 text is in [LICENSE](LICENSE).
The software is provided without warranty, including the implied warranties
of merchantability or fitness for a particular purpose.

This notice identifies the documented adaptation; upstream copyright and
license notices must be retained when redistributing corresponding source.

## Rust dependencies and embedded assets

Rust dependencies retain their own licenses and notices. The allowlist in
`deny.toml` is a dependency review policy, not a replacement for those terms.
In particular, bundled fonts carry their own font-license requirements.
Use `cargo metadata --locked --format-version 1` to inventory package licenses
and preserve the applicable notices/license texts when packaging releases.
