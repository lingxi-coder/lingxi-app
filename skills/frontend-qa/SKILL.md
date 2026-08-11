---
name: frontend-qa
description: Verify a generated local frontend through browser and native WebView inspection, covering navigation, console errors, core interactions, device context, and the required phone/tablet matrix.
---

# Frontend QA

Verify the running app, not only the build. Start with the available Browser
capability for the preview URL: inspect the console, capture the first view,
exercise the primary path, test navigation/back, and check loading, empty,
error, success, and permission states.

Then use the real Local Apps WebView tools (`inspect_ui`, `act_on_ui`, logs and
bridge responses) to verify host data, device operations, system back, and the
injected device context. Browser-only evidence is not evidence of bridge or
native behavior.

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
file to change. The orchestrator may perform at most two repair → rebuild →
retest rounds. After the second unsuccessful round, report remaining defects
honestly instead of hiding them.
