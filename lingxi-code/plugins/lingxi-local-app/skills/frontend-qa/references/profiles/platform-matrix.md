# Platform matrix

Run the same scenario across the requested targets and record what changed.

- Browser preview: first paint, fatal console errors, primary interaction,
  navigation, back, loading and error handling.
- iPhone: safe area, 44pt controls, iOS navigation/back, text scaling, reduced
  motion.
- Android phone: 48dp controls, Android back, Material feedback, permission and
  recovery flows.
- iPad portrait and landscape: resize, split/sidebar suitability, pointer,
  keyboard, and orientation changes.
- Android tablet portrait and landscape: rail/drawer or list-detail behavior,
  resize, pointer, keyboard, and system back.
- Desktop: resize, hover/focus, keyboard-first use, and long-width behavior.

If a target was not requested, say so instead of silently skipping it.
Record bridge-dependent checks, reduced-motion behavior, and at least one
double-frame motion observation for animated canvas or Three.js scenes.

## Sources

Reviewed: 2026-08-27

- Apple Human Interface Guidelines: https://developer.apple.com/design/human-interface-guidelines
- Android adaptive apps: https://developer.android.com/develop/adaptive-apps/guides/get-started-with-adaptive-apps
- WCAG 2.2: https://www.w3.org/TR/WCAG22/
