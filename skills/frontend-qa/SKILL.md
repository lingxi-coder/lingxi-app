---
name: frontend-qa
description: Verify a generated local frontend through browser and native WebView inspection, covering navigation, console errors, core interactions, device context, and the required phone/tablet matrix.
---

# Frontend QA

Verify the running app, not only the build. Start with the available Browser
capability for the preview URL: inspect the console, capture the first view,
exercise the primary path, test navigation/back, and check loading, empty,
error, success, and permission states.

Then use the real Local Apps WebView tools (`LocalAppInspectUi`,
`LocalAppActOnUi`, `LocalAppLogs` and bridge responses) to verify host data,
device operations, system back, and the injected device context. Browser-only
evidence is not evidence of bridge or native behavior.

## Canvas and WebGL surfaces

`LocalAppInspectUi` is a DOM snapshot built from a fixed set of element
selectors. A `<canvas>` matches none of them, so an app that draws its interface
returns an **empty element list whether it is rendering correctly, rendering
nothing, or has crashed**. An empty snapshot is not evidence; treating it as
"nothing rendered" or as "nothing broken" are both wrong.

For those apps:

- `LocalAppCaptureUi` returns the actual frame. This is the only way to
  distinguish a working render from a blank one. The image itself arrives as
  an image block; everything you need to read coordinates off it arrives
  beside it as flat fields — `image_width`/`image_height` (the frame's own
  pixels) next to `viewport` (CSS pixels). For a WHOLE-VIEW capture (no `rect`
  requested, see below), a feature at image `(ix, iy)` is at CSS
  `(ix * viewport.width / image_width, iy * viewport.height / image_height)`.
  Do not send an image coordinate as a pointer coordinate — the frame is
  downscaled, and a coordinate outside the viewport is rejected.
- `LocalAppCaptureUi` also takes an optional `rect` (`{x, y, width, height}`,
  CSS pixels) to crop to one region instead of the whole view — useful to
  read a small area at full resolution instead of spending the capture's
  fixed pixel budget on the rest of the screen. A rect that hangs off the edge
  of the screen is fine, including a NEGATIVE `x`/`y` — which is exactly what
  `LocalAppInspectUi` reports for an element scrolled above the fold — because
  the client clamps it to the viewport rather than refusing it. Only a
  zero-or-negative `width`/`height`, or a rect with no overlap with the
  viewport at all, is an error. When a crop was requested, the result
  additionally carries `capture_rect` (`{x, y, width, height}`, CSS pixels):
  the region actually captured AFTER clamping, which can differ from the
  requested `rect` if part of it was off-screen. `viewport` still reports the
  WHOLE view either way, so it cannot convert a cropped image's coordinates —
  use `capture_rect` in its place instead, AND add its origin back in, since a
  crop is not anchored at the view's `(0, 0)`:
  `(capture_rect.x + ix * capture_rect.width / image_width,
  capture_rect.y + iy * capture_rect.height / image_height)`. A smaller
  `image_width` than `viewport` implies is not itself evidence a crop happened
  — the whole-view capture already downscales — so branch on whether
  `capture_rect` is present in the result, not on the image being smaller than
  expected.
- **The crop `rect` is VIEW space; `LocalAppInspectUi`'s rects are layout
  CSS pixels. They are the same numbers only at zoom scale 1 with no visual
  offset.** `LocalAppInspectUi`'s `elements[].rect` and `canvases[].rect` are
  measured against the layout viewport, while `LocalAppCaptureUi` consumes
  `rect` (and reports `capture_rect`) in the native view's own coordinates.
  Pinch-zoom is on by default in the app WebView, and the shipped template
  sets only `initial-scale=1.0` — no `user-scalable=no`, no `maximum-scale` —
  so a user (or a stray gesture during QA) can put the page at a scale where
  feeding an inspect rect straight into a crop captures the wrong region, with
  no error. `LocalAppInspectUi`'s `viewport.scale`, `viewport.offsetLeft` and
  `viewport.offsetTop` are how you detect that: pass an inspect rect through to
  a crop only when `scale` is 1 and both offsets are 0, and otherwise capture
  the whole view and read coordinates off `viewport` instead of cropping.
- `LocalAppActOnUi` with `action: "pointer"` drives it — `value` is `"x,y"` or
  `"x,y,phase"` in CSS pixels, phase `tap` (default), `down`, `move` or `up`.
  `click` cannot reach a canvas: it resolves an element by selector/role/name,
  and a canvas matches none of them.
- `LocalAppActOnUi` with `action: "key"` sends keyboard input — `value` is
  `"<key>"` or `"<key>,phase"`, phase `press` (default), `down` or `up`. Use
  `down`/`up` to hold a key, which a game usually needs. Spell the space bar
  `Space`, not `" "` — a lone space cannot survive the wire, and `Space` is
  delivered as `key: " "` with `code: "Space"` so both spellings of a handler
  match.

Do not report a canvas app as verified on DOM evidence alone, and do not raise
DOM-shaped findings against it — a canvas has no landmarks, no tab order and no
measurable touch targets, and findings the app cannot act on consume repair
rounds that a real defect then cannot use.

## Matrix

Run the view at a representative viewport for each requested target, while
injecting the platform context separately: iPhone, Android phone, iPad portrait
and landscape, and Android tablet portrait and landscape. Do not infer a
platform from width. Check safe-area padding, touch target dimensions, the
platform navigation model, adaptive panes, orientation behavior, focus/hover,
dark/light, and reduced motion. Apply the checklist in
[references/verification-matrix.md](references/verification-matrix.md).

If Browser is unavailable, report the lower verification level and use the
existing native inspect/act/log path; never claim full visual Browser QA.

## Repair loop

Return deterministic findings with severity, evidence, and a concrete source
file to change. The orchestrator decides how many repair → rebuild → retest
rounds it allows — one for the `fast` and `balanced` strategies, two for
`thorough` — so do not assume a second round exists. After the last round,
report remaining defects honestly instead of hiding them.
