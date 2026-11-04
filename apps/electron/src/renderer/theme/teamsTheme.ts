// The Teams workspace (packages/teams) is themed through --teams-* variables with a light and a dark fallback of
// its own. The Desktop sets every one of them from its tokens, so the workspace follows the Desktop's appearance
// choice rather than the system's, and its colors are the Desktop's.

import type { Tokens } from './tokens';

const channels = (hex: string): [number, number, number] => [1, 3, 5].map(at => Number.parseInt(hex.slice(at, at + 2), 16)) as [number, number, number];

/** `weight` of `from` and the rest of `to`, both `#rrggbb`. */
export function mixColors(from: string, to: string, weight: number): string {
  const [a, b] = [channels(from), channels(to)];
  return `#${a.map((value, index) => Math.round(value * weight + b[index]! * (1 - weight)).toString(16).padStart(2, '0')).join('')}`;
}

/**
 * The public variables of `packages/teams/workspace.css`, from a palette. Pairs that carry text meet 4.5:1 and
 * outlines and fills 3:1 against the surfaces they sit on, in both palettes (see teams-desktop.test.ts).
 */
export function teamsThemeVariables(t: Tokens): Record<string, string> {
  return {
    '--teams-bg': t.stageBg,
    // The Desktop's sidebar panel, which the workspace's sidebar sits on without a seam.
    '--teams-sidebar-bg': t.sidebarPanelBg,
    '--teams-surface': t.surface,
    '--teams-fg': t.text,
    // A selected row is the darkest surface text sits on; this is the muted text that still reads there.
    '--teams-muted': t.text3OnActive,
    // Outlines of controls need 3:1, which the Desktop's hairline border does not give; the hairline is for dividers.
    '--teams-border': mixColors(t.text, t.surface, 0.55),
    '--teams-line': t.border,
    '--teams-hover': t.surfaceHover,
    '--teams-selected': t.surfaceActive,
    // The fill of a primary button: the accent, darkened as the Desktop's own primary buttons are, under white text.
    '--teams-accent': mixColors(t.accent, '#000000', 0.88),
    '--teams-accent-fg': '#ffffff',
    // The accent as text and as a line: deeper on the light palette, lighter on the dark.
    '--teams-accent-text': t.dark ? mixColors(t.accent2, '#ffffff', 0.75) : mixColors(t.accent, '#000000', 0.85),
    '--teams-danger': t.dark ? mixColors(t.danger, '#ffffff', 0.85) : t.danger,
    '--teams-badge-bg': t.danger,
    '--teams-badge-fg': t.dark ? '#1a0b0b' : '#ffffff',
    '--teams-online': t.dark ? t.ok : mixColors(t.ok, '#000000', 0.9),
    '--teams-focus': t.dark ? t.accent2 : t.accent,
    '--teams-font': 'var(--font-sans-default)',
  };
}
