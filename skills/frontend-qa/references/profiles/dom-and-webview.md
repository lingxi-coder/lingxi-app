# DOM and native WebView checks

- Verify navigation, back behavior, empty/loading/error/success/permission
  states, form validation, and recoverability.
- Inspect browser console and native logs separately; do not assume one mirrors
  the other.
- Confirm `window.lingxi.v2` bridge paths used by the app behave correctly for
  data, device, clipboard, files, network, or notifications in scope.
- On native, check safe areas, keyboard overlap, system back, dialogs/sheets,
  and orientation changes.
- If the app is routed, verify screen transitions and that each main route keeps
  its shell usable after forward and back navigation.

## Sources

Reviewed: 2026-08-27

- Ionic React navigation: https://ionicframework.com/docs/react/navigation
- WCAG 2.2: https://www.w3.org/TR/WCAG22/
