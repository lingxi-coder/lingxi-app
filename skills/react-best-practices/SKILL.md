---
name: react-best-practices
description: Generate maintainable React local apps with clear component boundaries, stable state, complete UI states, predictable effects, and production-safe performance patterns.
---

# React best practices

Keep the app a real, usable React project. Split the shell, feature regions,
state helpers, and repeated primitives into focused modules; keep `App` as
composition glue. Model loading, empty, error, success, and permission states
explicitly instead of hiding them behind optimistic placeholders.

Prefer derived values over duplicated state, stable keys over array indexes,
event handlers over effects for user actions, and cleanup for subscriptions,
timers, and bridge requests. Avoid render-time side effects, unnecessary
memoization, monolithic components, and platform conditionals scattered
through JSX. Put platform differences behind token/adapter modules supplied by
the confirmed design spec.

Use CSS or the Web Animations API only for purposeful motion and honor reduced
motion. Use accessible inline SVG or CSS for ordinary UI icons; do not turn
icons into generated bitmap assets. Load [references/react-checklist.md](references/react-checklist.md)
for a final pass. This skill does not install packages or change root
infrastructure; stay within the host-scaffolded dependency set for the current
local app.
