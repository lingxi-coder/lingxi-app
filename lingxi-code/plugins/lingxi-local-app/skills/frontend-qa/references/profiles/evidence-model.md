# Evidence-first workflow

- Start with browser verification for the preview URL: primary path, console,
  network-visible failures, and screenshots.
- Then verify the same flow through native WebView inspection and logs because
  bridge behavior, device context, and system back are host-specific.
- If Browser is unavailable, mark verification as degraded, continue with the
  native inspect/act/log path, and do not claim full visual Browser coverage.
- Record evidence before suggesting fixes. A claim without a screenshot, log
  line, console message, UI snapshot, or deterministic reproduction step is not
  a finding yet.
- Prefer one concrete finding with good evidence over many speculative issues.

For motion-heavy flows, capture two frames or observations across time so the
report can distinguish a static render from a broken transition.

## Sources

Reviewed: 2026-08-27

- LingXi Local Apps handoff: `docs/local-apps/HANDOFF.md`
