---
name: accessibility
description: Apply accessible frontend semantics and interaction behavior while generating a local app, including keyboard, screen reader, contrast, focus, touch targets, and motion preferences.
---

# Accessibility

Treat accessibility as part of the implementation contract. Use semantic
landmarks and headings, real labels for controls, meaningful names for icons,
visible focus, logical tab order, and status/error announcements. Preserve
keyboard and assistive-technology access to every core action.

Use the host platform's native context and tokens rather than hard-coded
assumptions. Keep controls at least 44pt on iOS and 48dp on Android, maintain
contrast for text and state changes, support Dynamic Type/system font fallback,
dark/light schemes, and honor `prefers-reduced-motion`/the injected reduced
motion preference. Do not communicate state by color alone.

Before handing off, inspect the generated UI for unlabeled controls, clipped
text at large type, focus traps, keyboard-only failures, inaccessible dialogs,
and touch targets. Load detailed checks from
[references/a11y-checklist.md](references/a11y-checklist.md) when the surface
is complex. This skill may edit generated source, but it does not manage npm
dependencies or redesign platform navigation.
