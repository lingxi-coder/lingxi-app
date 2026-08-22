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
  distinguish a working render from a blank one. The result carries
  `image.width`/`image.height` (the frame's own pixels) beside `viewport`
  (CSS pixels): a feature at image `(ix, iy)` is at CSS
  `(ix * viewport.width / image.width, iy * viewport.height / image.height)`.
  Do not send an image coordinate as a pointer coordinate — the frame is
  downscaled, and a coordinate outside the viewport is rejected.
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
