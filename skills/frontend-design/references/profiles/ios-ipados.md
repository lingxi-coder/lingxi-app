# iOS and iPadOS profile

Use Apple-shaped presentation, not generic web chrome.

- iPhone: favor hierarchical navigation, native-feeling top bars and tab bars,
  44pt targets, safe areas, Dynamic Type tolerance, swipe-back, sheets and
  popovers where appropriate.
- iPad: use sidebar or split/list-detail presentation, controlled content width,
  portrait and landscape behavior, pointer hover/focus, and keyboard shortcuts
  when the flow benefits from them.
- If both iPhone and iPad are targeted, write different `presentation` entries;
  do not stretch the phone shell.

Avoid:

- Material FAB/ripple patterns;
- phone-only single columns on iPad when the brief needs browsing/detail work;
- ignoring pointer and hardware keyboard behavior on iPad.

## Sources

Reviewed: 2026-08-27

- Apple HIG: https://developer.apple.com/design/human-interface-guidelines
- Tab bars: https://developer.apple.com/design/human-interface-guidelines/tab-bars
- Toolbars: https://developer.apple.com/design/human-interface-guidelines/toolbars
- Search fields: https://developer.apple.com/design/human-interface-guidelines/search-fields
