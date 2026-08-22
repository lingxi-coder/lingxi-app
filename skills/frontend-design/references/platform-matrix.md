# Local app platform matrix

Read the device context through the checked-in `lib/lingxi-bridge.js`
(`getDeviceContext`), which resolves `window.lingxi.v2.deviceContext`. It is
more authoritative than user-agent parsing and carries `os`, `formFactor`,
`viewport`, `safeArea`, `colorScheme`, `reducedMotion` and `inputMode`.

Not `window.lingxi.v1`: the v2 runtime is a direct cutover and deliberately
installs no v1 object, so that path is `undefined` on every current app rather
than merely older.

`viewport`, `safeArea`, `colorScheme`, `reducedMotion` and `inputMode` are LIVE
— they change on rotation, on iPad multitasking resize, and on an appearance
switch. Read them from the provider rather than caching one value at mount.

| Target | Required native cues | Explicitly avoid |
| --- | --- | --- |
| iOS / iPhone | Safe-area padding, 44pt touch targets, system-font/Dynamic Type fallback, hierarchical navigation, iOS top bar and bottom tabs when appropriate, swipe-back, dark/light and reduced-motion tokens | Material FAB/ripple or Android-shaped navigation |
| Android / phone | 48dp targets, Material 3 top app bar/navigation, state-layer/ripple, system back, dynamic-color compatible tokens, FAB only when the action warrants it | iOS tab/back conventions copied verbatim |
| iPad / tablet | Sidebar or split view, master-detail/multi-column layout, controlled content width, popovers/sheets, orientation changes, safe area, pointer hover/focus and keyboard shortcuts | A stretched iPhone column |
| Android tablet | Material 3 navigation rail/drawer, adaptive list-detail panes, multi-column orientation rules, keyboard/mouse/touch coordination | A stretched phone layout or iOS chrome |
| Desktop | Existing desktop shell, pointer/keyboard affordances, resizable content and desktop QA | Replacing mobile-native patterns with desktop chrome |

Keep shared data and business logic separate from these presentation adapters.
