# Router

Read this file first, then load only the profiles the confirmed brief needs.

## Output contract

- `surface: dom` -> read `profiles/dom.md`
- `surface: canvas` -> read `profiles/canvas-overlay.md`
- Any iPhone or iPad target -> read `profiles/ios-ipados.md`
- Any Android phone or tablet target -> read `profiles/android.md`
- Any desktop target -> read `profiles/desktop.md`

## Non-negotiables

- Write one `presentation` per target. "Responsive" is not a presentation.
- Keep `targets[]` explicit even when one source serves multiple platforms.
- Put platform deltas in navigation, chrome, density, spacing, and input, not
  only width breakpoints.
- Use platform system fonts and checked-in assets by default. Treat any other
  font or asset as an unresolved proposal; never imply a download or package
  that the locked Local App scaffold does not contain.
- If the app uses canvas, specify both the drawn-surface rules and the overlay
  rules; do not treat HUD/menu design like a normal web page.

## Sources

Reviewed: 2026-08-27

- Apple Human Interface Guidelines: https://developer.apple.com/design/human-interface-guidelines
- Android adaptive apps: https://developer.android.com/develop/adaptive-apps/guides/get-started-with-adaptive-apps
- Android adaptive navigation: https://developer.android.com/develop/adaptive-apps/guides/build-adaptive-navigation
- Ionic React navigation: https://ionicframework.com/docs/react/navigation
