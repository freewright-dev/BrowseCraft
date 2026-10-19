# Proposal: ad blocking in BrowseCraft

| | |
| --- | --- |
| **Status** | Ad blocker removed from the tree. Decision needed on what, if anything, replaces it. |
| **Date** | 2026-10-17 |
| **Scope** | Why a uBlock Origin (uBO) port, or any ad blocking, does not work with BrowseCraft's current architecture, and what we could do instead. |

## 1. Summary

We built an ad blocker modelled on uBlock Origin: a filter engine (EasyList/uBO
syntax, network rules plus `##` element hiding), an injected page script, and, in
the second round, a native WebView2 request interceptor and YouTube-specific
"scriptlets". We have now removed all of it.

The short version of why:

1. **uBO is a browser extension, not a library.** Its power comes from extension
   APIs (blocking every request before it is sent, injecting scripts at the very
   start of every frame, running code in the page's own context). BrowseCraft
   embeds a web view through `wry`, which gives us none of those on all
   platforms. On Windows we can reach them only through raw COM calls, on macOS
   essentially not at all, and on Linux there is no web view (the engine
   renders everything).
2. **Video ads are data, not requests.** YouTube describes its ads inside its
   own player JSON. Removing them means rewriting that JSON in flight, which uBO
   does with scriptlets that its maintainers rewrite every few weeks as YouTube
   changes. That is a standing maintenance job, not a feature we can finish.
3. **We cannot test it.** The only environment where the Windows path runs
   (WebView2) was not available while this was built. The first version never
   blocked a single ad in the real app because of a crash we could not see, and
   the second version's native hook and YouTube logic were verified only
   against a simulation.
4. **Licensing is unresolved.** uBO and its filter lists are copyleft
   (verify exact terms before shipping), and this repository declares no
   licence at all.

The filter *matching* turned out to be the easy part (it is fast and correct).
The *integration surface* is what we do not have.

**Recommendation:** pause ad blocking. If it comes back, start with the cheap,
time-boxed spike in section 6.1 (load the real uBO as a WebView2 extension),
because it removes the porting and maintenance burden entirely, and treat
everything else as optional. Section 7 lists non-ad-blocking directions that
reuse what we already have.

## 2. What we built and what happened

### Round 1: a uBO-style engine in `adblock.rs`

* Parser and matcher for network filters (`||host^`, `@@` exceptions, `$` options
  such as `third-party`, `domain=`, request types) and `##` cosmetic filters.
* Four sinks: navigation/pop-up checks in Rust, an injected script for WebView2
  pages, engine-page stylesheet filtering, and an engine-page cosmetic
  stylesheet.
* Result: no ads were blocked on real sites. Diagnosis (by reading the code):
  the injected script ran at document creation, where neither `document.head`
  nor `document.documentElement` exists. It threw
  `TypeError: Cannot read properties of null (reading 'appendChild')` while
  inserting its element-hiding `<style>`, which aborted the whole script before
  any network hook was installed. We reproduced the abort in a Node simulation;
  we could not reproduce it in a real WebView2.

### Round 2: fixes and a native interceptor

* Fixed the crash (constructable stylesheet; every section isolated).
* Added a native `WebResourceRequested` handler for WebView2, because the page
  script can never see tags the HTML parser inserts (`<script src>`, `<img>`,
  `<iframe>` written in the page's HTML).
* Added token indexing so ~66,000 rules stay fast, honoured `$third-party` and
  `$domain=` in JS, kept every host of multi-host cosmetic rules, bundled
  EasyList, EasyPrivacy and uBO's lists.
* Added YouTube "scriptlets": prune `adPlacements`, `playerAds`, `adSlots` from
  `ytInitialPlayerResponse`, `JSON.parse`, `fetch` and XHR, plus a runtime ad
  skipper.

### What was and was not verified

| Claim | How it was checked | Verified on a real WebView2? |
| --- | --- | --- |
| Matching engine is correct | 27 unit tests, including a token-index-vs-brute-force equivalence test | n/a (pure Rust) |
| Matching engine is fast | 66,516 network rules parse in ~90 ms; ~17 µs per lookup (release build, Linux) | n/a |
| Injected script no longer crashes at start | Node simulation with `documentElement = null` | **No** |
| Native interceptor compiles and blocks | API signatures checked against the `webview2-com` 0.38 / `wry` 0.53 sources; the Windows-only function could **not** be compiled here | **No** |
| YouTube pruning removes ads | Node simulation of the player data, `fetch`, XHR and the player DOM | **No** |

Nobody has seen the app block an ad. Everything above the matching engine is
unproven in the real environment.

## 3. Why this does not work with the current system

### 3.1 What uBO actually is

uBlock Origin is a browser extension. Its filtering is built on capabilities
only extensions get:

| What uBO needs | Extension API it uses | What BrowseCraft has |
| --- | --- | --- |
| Judge every request (including parser-inserted tags, other frames, workers) before it is sent | `webRequest` blocking | **Windows:** WebView2 `WebResourceRequested` via raw COM (we wrote this, untested). **macOS:** `wry` exposes no per-request hook (WKWebView content rule lists are not exposed). **Linux:** no web view. |
| Run code before any page script, in every frame, in the page's own JS world | Content scripts at `document_start`, `MAIN` world | `with_initialization_script` (runs, but at a moment when `document.head`/`documentElement` do not yet exist, which is how round 1 broke) |
| Rewrite network responses (to strip ad data) | Scriptlets wrapping `fetch`/XHR/`JSON.parse` | Only by wrapping those functions ourselves in the init script |
| Keep the rules fresh | Filter-list auto-updater | None. Bundled lists go stale the day they are copied. |
| Neuter scripts instead of blocking them (so pages do not break) | Redirect resources (`$redirect`) | Not implemented; such rules are dropped |

### 3.2 The renderer split

BrowseCraft renders internal pages (`about:`, `view-source:`, `browsecraft://`)
with its own engine and everything on the web with WebView2 (Windows) or
WKWebView (macOS). That is the right design for a small browser, but it means:

* The part of the system we fully control (the engine and `net.rs`) is the part
  that shows almost no ads: it runs no JavaScript, and ads are overwhelmingly
  injected by JavaScript.
* The part that shows the ads is a web view we configure but do not own. We can
  only use the hooks `wry` and the platform expose.

### 3.3 In-page JavaScript alone cannot be an ad blocker

Hooking `src` setters, `fetch` and XHR from an init script misses everything the
parser creates, runs after the page's own early scripts can already have cached
the original functions, and can be bypassed or broken by the page. This is why
real blockers intercept in the network layer, and why we added the native
`WebResourceRequested` handler. That handler exists only on Windows.

### 3.4 Video ads (YouTube and similar)

Pre-roll ads are not separate requests to an ad server that a rule can block.
YouTube places the ad description (`adPlacements`, `playerAds`, `adSlots`) in the
same player response that describes the video, so a network rule cannot remove it
without breaking playback. uBO's own filter set handles this with scriptlets
(`set-constant` on `ytInitialPlayerResponse`, `json-prune`,
`json-prune-fetch-response`, `json-prune-xhr-response`, later
`trusted-replace-*`). Its issue tracker shows the rules being rewritten again and
again as YouTube changes its delivery (examples visible in the
`uBlockOrigin/uAssets` issues #20315, #25399, #26138, #26515).

Two further problems no amount of client work removes:

* **Server-side ad insertion (SSAI):** ads stitched into the video stream
  itself. Industry write-ups in 2026 describe this as YouTube's biggest
  long-term threat to ad blocking, because the ad is indistinguishable from the
  content at the network level. The only client-side response is to mute and
  fast-forward, which is what our skipper did, and that is an arms race.
* **Anti-adblock enforcement:** YouTube detects blockers and can interrupt
  playback with a warning dialog. Countermeasures need constant attention.

### 3.5 Maintenance cost

The code that blocks YouTube today may not work next month. uBO stays effective
because several maintainers ship filter and scriptlet updates continuously. For a
small project that is an unbounded commitment: a bundled static list would
silently rot, and an updater adds a download path, a trust question (who signs
the lists?) and more code to secure.

### 3.6 Verification gap

Only the Windows path matters for real web pages, and we could not run it while
developing this. Each Windows-side change was written against the crate sources
and checked by simulation. For a feature whose failure mode is "silently does
nothing" (which is exactly what happened in round 1), that is not enough
assurance. Any future attempt needs a Windows test loop first: a machine with
WebView2, a set of known-ad test pages, and an automated check that a blocked
request really never leaves the browser.

### 3.7 Licensing and policy

* uBlock Origin is GPLv3 and its `uAssets` filter lists are published under
  copyleft terms; EasyList/EasyPrivacy are dual-licensed (CC BY-SA 3.0 /
  GPLv3). The exact terms must be checked before redistributing any of them.
* This repository currently has **no `LICENSE` file and no licence field in
  `Cargo.toml`**. Bundling or porting copyleft code and lists forces a licence
  decision we have not made.
* Blocking YouTube ads works against YouTube's business model and its terms of
  service, and YouTube actively counters it (see 3.4). That is a product and
  legal decision, not just a technical one.

## 4. What was removed

All of the following was deleted from the project tree (starting again from your
original upload, so none of the round-2 changes carry over):

* `src/adblock.rs` (the engine, the injected JavaScript, the YouTube scriptlet).
* The `filters/` directory (starter list, plus the EasyList, EasyPrivacy, uBO
  and YouTube lists added in round 2) and its copy step in `build.bat`.
* In `src/loader.rs`: the `about:blocked:` notice page and its guard
  (`blocked_notice_target`, `ad_blocked_page`), the engine cosmetic stylesheet,
  the stylesheet/image filtering, the view-source guard and two tests.
* In `src/browser.rs`: the main-frame block, the pop-up block and its status
  notice, the WebView2 navigation handler, the injected initialization script,
  and the `about:blocked` test cases.
* In `src/address.rs` and `src/main.rs`: the `about:blocked` test case and the
  `mod adblock;` line.
* Documentation: the README sections, `docs/usage.md`, `docs/architecture.md`,
  `docs/development.md`. The changelog's old "Added" entry was replaced by a
  "Removed" entry pointing at this file.

The `windows` dependency added in round 2 for the native interceptor is not
present (the project was rebuilt from your original upload). Your original upload still
contains the round-1 blocker; the round-2 archive (`Prowl-adblock-fixed.zip`) is
the only copy of the round-2 work. Keep both, or commit them to a branch, if you
want a reference.

## 5. Options for the future

Effort: **S** = about a day, **M** = a week, **L** = several weeks.

### 5.1 Load the real uBlock Origin into WebView2 as an extension (Windows)

WebView2 can load unpacked browser extensions
(`ICoreWebView2Profile7::AddBrowserExtension`, introduced in WebView2 SDK
1.0.2210.55; it requires the environment option for extensions to be enabled).
`wry` 0.53.5, already a dependency, exposes this on Windows as
`with_browser_extensions_enabled(true)` and `with_extensions_path(path)` (from
`WebViewBuilderExtWindows`). No porting: uBO's own request blocking, scriptlets
and updater would run unmodified.

| | |
| --- | --- |
| Blocks | Whatever uBO blocks, including its YouTube scriptlets, if the extension runs |
| Effort | **S** to test, **S-M** to ship (bundling, first-run, settings toggle) |
| Platforms | Windows only |
| Unknowns (must be tested) | Whether the current WebView2 runtime accepts uBO. Chromium has been retiring Manifest V2 extensions; the Microsoft pages we found do not say which manifest versions WebView2 supports. uBO's MV3 variant, uBO Lite, is more limited and its fixes are gated by store review. Microsoft also notes extensions load without their UI (no toolbar button or popup), so uBO would have no settings UI. `wry` ignores the completion result of `AddBrowserExtension`, so failures are silent. |
| Risks | GPLv3 redistribution of the extension; dependence on a WebView2 feature that could change |
| Verdict | **Best first step.** Cheap, and it either works or tells us quickly that this route is closed. |

### 5.2 Embed Brave's `adblock` crate in place of our own engine

`adblock` (crate `adblock` 0.13.x, MPL-2.0, from the Brave project) is a
maintained Rust engine: network blocking, cosmetic filtering, uBO syntax
extensions, and a `resource-assembler` feature that reads uBO's scriptlet and
redirect resources. Firefox's tracking-protection work uses a fork of it. It would
replace our hand-written parser and matcher with a better-tested one.

| | |
| --- | --- |
| Blocks | Network and cosmetic rules; scriptlets and redirects if we wire the resources in |
| Effort | **M** to swap the matcher; **L** to also deliver scriptlets and keep resources current |
| Platforms | Engine is portable; interception is still Windows-only (section 3.1) |
| Risks | Does not solve the integration problem (interception, init-script timing, YouTube upkeep); needs a list/resource updater; adds a dependency |
| Verdict | Worth it only if we decide to own an in-house blocker. Not a fix on its own. |

### 5.3 WebView2 tracking prevention

WebView2 profiles have a `PreferredTrackingPreventionLevel` setting (Microsoft's
tracker blocking, with several strength levels). It is reachable through the COM
bindings we already use (`webview2-com-sys` exposes the profile call).

| | |
| --- | --- |
| Blocks | Known trackers (a privacy feature). **Not** ads in general and **not** video ads |
| Effort | **S** |
| Platforms | Windows only |
| Risks | Easy to oversell: it must be presented as tracking protection, not ad blocking |
| Verdict | Cheap, honest privacy win. Pair it with a settings entry in `browsecraft://flags`. |

### 5.4 Chromium host-resolver rules (domain blocking without page code)

Chromium has a `--host-resolver-rules` switch that maps hostnames to nothing.
`wry` lets us pass extra browser arguments on Windows
(`with_additional_browser_args`). This blocks a domain for every request,
including parser-inserted tags and other frames, with no interception code. This
comes from Chromium's documented switches and has **not** been tested here.

| | |
| --- | --- |
| Blocks | Whole domains only |
| Effort | **S-M** |
| Platforms | Windows only |
| Limits | The browser command line has a length limit (about 32,000 characters on Windows), so only a few hundred to a few thousand hosts fit, not 60,000. No cosmetic filtering, nothing for YouTube (the ads share its domains). |
| Verdict | Possible stopgap for a short "worst offenders" list; weak as a product. |

### 5.5 Reader mode in the engine

A "reader view" that extracts the main article and renders it in BrowseCraft's
own engine, with no ads, scripts or layout clutter. It uses the part of the
system we control and test, works on every platform (including Linux), and has
no arms race.

| | |
| --- | --- |
| Blocks | Ads and clutter on article pages, by not rendering them |
| Effort | **M** (content extraction heuristics, a toolbar toggle, tests on saved pages) |
| Platforms | All |
| Limits | Articles only; not video or app-style sites |
| Verdict | Strong "different angle": delivers the user-visible benefit without chasing ad networks. |

### 5.6 Recommend DNS-level blocking outside the browser

Document that users can run AdGuard Home, NextDNS or a Pi-hole. No code, works
for every app. Limits: cannot separate YouTube video ads from video (shared
domains) and does nothing for SSAI.

| | |
| --- | --- |
| Effort | None (a documentation note) |
| Verdict | Fine as guidance; not a feature. |

### 5.7 Do nothing

Keep BrowseCraft ad-block free and say so. Pop-ups are already denied and
reopened in the current tab; the engine renders no scripts; that is already a
quieter experience than a default browser.

## 6. Comparison and recommendation

| Option | Blocks ads | Blocks video ads | Platforms | Effort | Ongoing upkeep | Main risk |
| --- | --- | --- | --- | --- | --- | --- |
| 5.1 Real uBO as extension | Yes, if it loads | Yes, if it loads | Windows | S | Low (uBO updates itself) | WebView2 may not run it; GPL |
| 5.2 `adblock` crate | Network + cosmetic | Only with scriptlet resources | Windows (interception) | M-L | High | Same integration gaps |
| 5.3 Tracking prevention | Trackers only | No | Windows | S | None | Mislabelling |
| 5.4 Host-resolver rules | Listed domains only | No | Windows | S-M | Medium | Command-line size |
| 5.5 Reader mode | Articles | No | All | M | Low | Extraction quality |
| 5.6 DNS guidance | Domain level | Partly | Any | None | None | Limited effect |
| 5.7 Nothing | No | No | n/a | None | None | Users expect more |

### 6.1 Proposed next step (time-boxed)

Spike 5.1 for one day, on a Windows machine with WebView2, with these pass/fail
checks:

1. Pass `with_browser_extensions_enabled(true)` and `with_extensions_path(..)`
   to the builder, with an unpacked uBO in that folder.
2. Confirm the extension is listed (read back `GetBrowserExtensions`, since
   `wry` hides load failures).
3. Open a known test page with ad slots and confirm ads and requests disappear
   (check the network log).
4. Try one YouTube video with a pre-roll.
5. Record the WebView2 runtime version and which uBO build (MV2 or Lite) worked.

If it passes, ship it behind a settings toggle with the licence notice. If it
fails, close ad blocking and spend the time on 5.3 and 5.5.

## 7. Other directions that reuse the existing system

These do not involve ads and fit the work already in the project:

* **Reader mode** (5.5), the most natural fit for the engine.
* **Privacy defaults:** WebView2 tracking prevention (5.3), HTTPS-first
  navigation, and a flag for third-party cookie policy.
* **Engine quality:** the engine is the part we own end to end; layout and CSS
  coverage improvements are fully testable on any machine.
* **Quality of life:** find-in-page, saved sessions, per-site zoom, download
  handling.

## 8. Decisions needed

1. Do we run the 5.1 spike, or close ad blocking now?
2. Which licence does the project want? (This blocks any redistribution of
   uBO, EasyList or uAssets content.)
3. Which "other angle" do we take first (section 7)?

## Appendix: sources

* uBlockOrigin/uAssets issues #20315, #25399, #26138, #26515 (YouTube ad
  filter history and the scriptlets used).
* EasyList wiki, "Youtube Issues": pre-roll and mid-roll rules.
* uBlock Origin wiki, "Resources Library": scriptlet reference
  (`set-constant`, `json-prune`, `json-prune-fetch-response`,
  `json-prune-xhr-response`, and the `trusted-*` family).
* Microsoft Learn, `ICoreWebView2Profile7` and `ICoreWebView2BrowserExtension`
  (WebView2 extensions, SDK 1.0.2210.55); `CoreWebView2Profile`
  (`PreferredTrackingPreventionLevel`).
* `wry` 0.53.5 source: `WebViewBuilderExtWindows::with_browser_extensions_enabled`,
  `with_extensions_path`, `with_additional_browser_args`.
* `brave/adblock-rust` and the `adblock` crate on crates.io (MPL-2.0).
* 2026 articles on YouTube server-side ad insertion and Manifest V3 effects on
  uBlock Origin (AdGuard, Supercharge, All Things VPN, UnboxFuture).
