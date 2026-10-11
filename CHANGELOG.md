# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Browser window that uses the system web engine through wry and tao.
- Toolbar with back, forward, reload/stop and home buttons, plus a loading indicator.
- Address bar that accepts full URLs, bare domains, `localhost` and IPv4 addresses, and falls back to a DuckDuckGo search.
- Optional URL or search as a command-line argument.
- Keyboard shortcuts: `Ctrl+L`, `F5`/`Ctrl+R`, `Alt+Left`/`Alt+Right` and `Alt+Home`.
- Page title shown in the window title.
- Pop-ups and `target="_blank"` links open in the current view.
- Light and dark toolbar theme that follows the system setting.
- Size-optimized release profile (binary under 500 KB).
- Documentation in `docs/`.
