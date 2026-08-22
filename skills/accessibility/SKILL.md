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

## Drawn surfaces (canvas / WebGL)

The checks above are DOM checks. A canvas has no landmarks, no headings, no tab
order and no measurable control rectangles, so running them against one produces
findings the app cannot act on — and an unresolved finding fails the whole
build, spending a repair round a real defect then cannot use. Do not raise
DOM-shaped findings against a drawn surface.

It still has an accessibility contract, just a different one:

- the canvas element carries a real label and a text description of what it
  shows, so a screen reader announces something other than "canvas";
- state that only exists as pixels — score, lives, level, game over — is also
  published in a live region, because a screen reader cannot read a drawing;
- every action is reachable without a pointer, since a pointer-only game is
  unplayable for anyone using switch control or a keyboard;
- state is never communicated by color alone (this one carries over unchanged,
  and matters more here: color is often all a drawn surface has);
- `prefers-reduced-motion` reduces or stops non-essential animation rather than
  being ignored because "it's a game".
