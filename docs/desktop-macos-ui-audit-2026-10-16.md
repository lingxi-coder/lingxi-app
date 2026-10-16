# Desktop macOS UI audit — 2026-10-16

Scope: `apps/electron`. Reviewed against the apple-design skill and Apple's
[macOS guidance](https://developer.apple.com/design/human-interface-guidelines/designing-for-macos/),
[menus](https://developer.apple.com/design/human-interface-guidelines/menus),
[materials](https://developer.apple.com/design/human-interface-guidelines/materials), and
[accessibility guidance](https://developer.apple.com/design/human-interface-guidelines/accessibility).

## Findings corrected

| Area | Finding | Change |
| --- | --- | --- |
| Window/sidebar | Duplicate brand/header inherited a horizontal flex basis in a vertical layout, leaving hundreds of pixels above the chat list. | Keep the single window toolbar; remove the redundant sidebar header. |
| Menu bar | No application-specific native menu was installed. | Add Settings (Command-comma), Activity, native Edit, View/zoom/fullscreen, Window, Services, Hide, About and Quit roles. Production excludes developer tools. |
| Direct manipulation | Terminal height transitions continued during pointer resizing. | Freeze the current displayed height on grab and disable transitions until release/cancellation. Explicit border-box sizing keeps measured and assigned heights consistent. |
| Materials | Reduced transparency skipped command/mention and project/session menus. | Remove backdrop blur and use opaque theme surfaces in reduced-transparency and increased-contrast modes. |
| Adaptive layout | Task list/detail breakpoint measured the window, ignoring width occupied by the sidebar. | Switch to the detail-only presentation when the task pane itself is at most 720px wide. |
| Controls | Scrollbar thumbs used a fixed dark tint; selects did not share the global system-font rule. | Derive scrollbar tint from foreground color and inherit the system font for selects. |

## Coverage and evidence

| Surface | Evidence |
| --- | --- |
| Window chrome | Source review: native hidden-inset titlebar, traffic-light placement, drag/no-drag regions. Renderer captures verify a single toolbar and sidebar content starting 16px below its surface. |
| Sidebar and sessions | Electron pointer tests cover hover, keyboard focus, clicks, holds and drag/drop; archive tests cover confirmation and focus restoration. |
| Typography and appearance | System font stack, optical sizing, theme tokens and system appearance subscription reviewed; light/dark renderer captures inspected. |
| Composer | Long draft, contained scrolling, send focus, minimum-window bounds; primary send contrast measured at 6.93:1 light and 4.84:1 dark. |
| Menus | Model picker placement and mention/command keyboard interaction; reduced-transparency computed-style regression; native menu template dispatch tests and renderer Settings event/focus check. |
| Settings | Existing Electron suite covers content/scroll-track insets, focus traversal, background dragging exclusion, managers, layered forms and provider drafts. General settings captured in both appearances. |
| Scheduled tasks | CRUD/date-picker interaction regression plus 600px pane in a wide window; full renderer at 900×600 verifies task detail fits beside the expanded sidebar. |
| Terminal | Real Electron pointer input verifies 40px pointer movement produces 40px height change with zero transition, followed by cleanup on release. Existing keyboard, shell retention, focus and input checks also pass. |
| Notifications | Electron appearance/dismissal/read/preferences regression and native-notification presentation tests. |
| Accessibility | Both appearances checked with emulated reduced motion, reduced transparency and increased contrast. Minimum renderer window 900×600 and standard 1280×850 captured. |
| Auxiliary panes | Source review of Side Chat, Runtime Center and Git Review layout/styles; no exhaustive live-state acceptance claimed. |

Validation: TypeScript checks and production Electron/Vite build; 21 focused unit
tests; the 26-test selected Electron suite (terminal passed on its focused rerun);
updated mention and scheduled-task regressions passed on their focused rerun.
The capture script completed with no renderer errors and 20 successful checks.

Reproduce renderer captures:

```sh
cd apps/electron
node scripts/capture-apple-desktop.mjs /tmp/lingxi-macos-audit-final
```

Captures and computed-style evidence are in `/tmp/lingxi-macos-audit-final` for this
run. These use real Electron rendering with a synthetic bridge. They do not prove
signed-package behavior, OS menu-bar mouse interaction, VoiceOver output, native
vibrancy, every possible tool transcript, or all live-provider states. This is an
evidence-bounded UI audit, not an assertion of complete HIG certification.
