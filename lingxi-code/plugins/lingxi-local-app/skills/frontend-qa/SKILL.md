---
name: frontend-qa
description: Verify LingXi local apps with evidence-first QA across Browser, native WebView, DOM, Canvas 2D, Three.js, Phaser, and Babylon runtime profiles.
---

# Frontend QA

Verify the running app, not only the build output.

Honor a machine-readable result schema supplied by the caller; do not wrap,
rename, or omit its required top-level fields. For standalone QA, return a
structured `qa_report` that records:

- target, surface, and runtime profile when supplied;
- pass/fail checkpoints;
- severity-scored findings;
- concrete reproduction steps;
- evidence from Browser, native WebView, console, logs, captures, or data;
- likely source file or subsystem to change next.

Routing is load-mode aware: bundled runtimes must use the `Bundled resource`
section below and must not read from the app workspace (`references/router.md`
or its profiles). File-backed runtimes follow the markdown link
[references/router.md](references/router.md) first, then only the platform and
surface profiles that match the app. Use only LingXi Browser and Local App tools
already provided by the product.
Report defects but do not repair source in the QA pass; the orchestrator owns
repair, rebuild, and retest rounds. When Host returns a
`verification_scope`, it is authoritative: exercise only the current-device
`in_scope_target_ids`, preserve `declared_target_ids`, and report
`unverified_target_ids`/`unverified_scenario_ids` explicitly. Never describe
untested targets as passed or summarize a partial run as full-matrix success.
Carry Host upstream finding IDs/messages unchanged. A post-repair pass may
resolve one only with exact fresh Host evidence IDs; it must not erase or
rewrite an immutable prior candidate.
