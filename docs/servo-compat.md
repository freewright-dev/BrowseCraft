# Servo compatibility run

First compatibility run of the experimental Servo web view
([issue #65](https://github.com/freewright-dev/BrowseCraft/issues/65)): 50
websites opened in BrowseCraft with Servo, looking at what the first screen
shows. It measures where the slice stands; it is not a pass/fail gate by
itself.

## Method

- **Build:** BrowseCraft debug build with `--features servo` (Servo 0.6.0 LTS),
  2026-10-19.
- **Machine:** WSL2 Ubuntu 24.04 (WSLg display, Mesa software rendering),
  16 cores, 24 GB of memory.
- **Window:** 1060×722 page area, dark theme.
- **Per site:** 30 seconds. `BROWSECRAFT_SERVO_TRACE` logged load progress,
  and `BROWSECRAFT_SERVO_FRAME` saved Servo's newest frame.
- **Rating:** done by eye from the saved frame.
  - *Works:* the page renders as intended; small icon glitches are allowed.
  - *Partial:* usable but visibly incomplete.
  - *Broken:* blank, or the site's own error page.
- **Not covered:**
  - Interaction: clicking, typing, scrolling and video playback. Media is
    built without GStreamer, so video can't play.
  - Logged-in pages.
  - Release-build timing.
- **Missing glyphs:** WSL has no fonts for some scripts (CJK, Devanagari), so
  a few lines show boxes. That is the test machine, not Servo.

Rerun with [tools/servo-compat/run.sh](../tools/servo-compat/run.sh); the site
list is [tools/servo-compat/sites.txt](../tools/servo-compat/sites.txt).

## Summary

| | Sites |
|---|---|
| Works | 33 |
| Partial | 6 |
| Broken | 11 |
| Crashed or hung the browser | 0 |
| Reached "load complete" within 30 s | 47 (median 1.5 s, 90 % within 10 s, slowest 20.9 s) |

Patterns:

- **Cookie banners and consent dialogs work.** Those from Google, YouTube,
  BBC, Guardian, Reuters, CNN, Instagram, X and Dropbox all render.
- **Icons often show as solid squares.** This affects small icons on
  Wikipedia, SRF, MDN and crates.io, which suggests icons drawn with CSS
  masks or SVG are not supported yet.
- **Most failures are large script-heavy apps.** GitHub repository pages,
  BBC, Figma, Netflix, Spotify and Amazon report a client-side error or stay
  blank.
- **Bot checks block some sites.** Stack Overflow stays stuck at a Cloudflare
  check. eBay may be refusing Servo's user agent rather than a missing
  feature.

## Results

Load time is when Servo reported the page complete, counted from Servo's
start (debug build). "–" means not within 30 s.

| # | Site | Result | Load (s) | Notes |
|---|---|---|---|---|
| 1 | example.com | Works | 0.66 | |
| 2 | www.wikipedia.org | Works | 0.91 | |
| 3 | en.wikipedia.org/wiki/Rust_(programming_language) | Works | 4.99 | Small icons render as squares |
| 4 | github.com | Works | – | Logo and hero artwork missing; never reported complete |
| 5 | github.com/servo/servo | Broken | 2.25 | "Unable to load page" |
| 6 | servo.org | Works | 0.75 | |
| 7 | duckduckgo.com | Works | 0.90 | |
| 8 | www.startpage.com | Works | 2.63 | |
| 9 | www.google.com | Works | 1.19 | Consent page |
| 10 | www.bing.com | Works | 1.93 | |
| 11 | news.ycombinator.com | Works | 1.28 | |
| 12 | www.reddit.com | Works | 1.94 | |
| 13 | old.reddit.com | Partial | 0.80 | Header only, body blank |
| 14 | www.youtube.com | Partial | 9.98 | Consent dialog and page frame; no video grid |
| 15 | www.bbc.com/news | Broken | 1.50 | Consent dialog over "Application error: a client-side exception" |
| 16 | www.nytimes.com | Works | 10.92 | Empty ad slot at the top |
| 17 | www.theguardian.com/international | Works | 5.89 | Consent dialog |
| 18 | www.cnn.com | Works | 3.75 | Consent dialog |
| 19 | www.reuters.com | Works | 3.00 | Consent dialog |
| 20 | www.srf.ch | Works | 2.28 | Small icons render as squares |
| 21 | www.admin.ch | Works | 3.43 | |
| 22 | www.amazon.com | Broken | 0.43 | Blank page |
| 23 | www.ebay.com | Broken | 0.78 | eBay error page |
| 24 | www.booking.com | Broken | 20.89 | Blank page |
| 25 | stackoverflow.com/questions | Broken | 11.20 | Stuck at Cloudflare's bot check |
| 26 | developer.mozilla.org/en-US/ | Works | 0.75 | Small icons render as squares |
| 27 | docs.rs | Works | 0.94 | |
| 28 | crates.io | Works | 0.79 | Small icons render as squares |
| 29 | www.rust-lang.org | Partial | 0.80 | Hero text barely visible |
| 30 | doc.rust-lang.org/book/ | Works | 0.76 | |
| 31 | www.python.org | Works | 0.54 | |
| 32 | nodejs.org | Partial | 1.00 | Heading drawn on a white box |
| 33 | www.w3.org | Works | 0.44 | |
| 34 | www.mozilla.org | Works | 1.36 | Logo artwork glitch |
| 35 | www.apple.com | Works | 1.11 | |
| 36 | www.microsoft.com | Partial | 14.66 | Header and chat widget only, body blank |
| 37 | www.cloudflare.com | Works | 4.20 | |
| 38 | x.com | Works | 1.04 | Login page and consent banner |
| 39 | www.instagram.com | Works | 6.11 | Consent dialog |
| 40 | www.linkedin.com | Works | 1.95 | |
| 41 | www.twitch.tv | Works | 11.57 | Player area black (no media support in this build) |
| 42 | www.netflix.com | Broken | 2.96 | Netflix error page (NSES-UHX) |
| 43 | open.spotify.com | Broken | 1.47 | "Something went wrong" |
| 44 | www.openstreetmap.org | Works | 0.87 | Map tiles load |
| 45 | archive.org | Partial | 3.54 | Donation banner and header, body blank |
| 46 | www.imdb.com | Broken | 0.61 | Blank page |
| 47 | www.paypal.com | Broken | – | Blank page; never reported complete |
| 48 | www.dropbox.com | Works | 3.49 | Consent banner |
| 49 | www.notion.so | Works | – | Some placeholders; never reported complete |
| 50 | www.figma.com | Broken | 1.60 | "Application error: a client-side exception" |

## Screenshots

The first screen of each site, reduced to half size:

![Sites 1–6](servo-compat/sites-01-06.jpg)
![Sites 7–12](servo-compat/sites-07-12.jpg)
![Sites 13–18](servo-compat/sites-13-18.jpg)
![Sites 19–24](servo-compat/sites-19-24.jpg)
![Sites 25–30](servo-compat/sites-25-30.jpg)
![Sites 31–36](servo-compat/sites-31-36.jpg)
![Sites 37–42](servo-compat/sites-37-42.jpg)
![Sites 43–48](servo-compat/sites-43-48.jpg)
![Sites 49–50](servo-compat/sites-49-50.jpg)
