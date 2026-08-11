# Verification matrix

For each target record platform context, viewport, orientation, and result.

| Target | Viewport examples | Assertions |
| --- | --- | --- |
| iPhone | 393×852 and 390×844 | 44pt controls, safe area, iOS navigation/back/swipe semantics |
| Android phone | 412×915 and 360×800 | 48dp controls, Material state layer, system back and dynamic color |
| iPad | 834×1194 portrait and 1194×834 landscape | sidebar/split or master-detail, controlled reading width, pointer/keyboard support |
| Android tablet | 800×1280 portrait and 1280×800 landscape | rail/drawer, adaptive list-detail panes, Material styling, pointer/keyboard/touch |
| Desktop | existing supported desktop viewport | desktop shell and core workflow remain usable |

Use the same business scenario for all targets, then assert that navigation,
touch sizing, safe-area handling, and layout tokens differ where the platform
requires it. Record browser console errors and native bridge/log outcomes.
