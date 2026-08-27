// ─── DESIGN TOKENS — Lingxi Code dark/light ─────────────────
// Ported verbatim from the design prototype's `tokens(dark)` factory.
// Colors are CSS oklch(...) values and are kept exactly as-is.

import type { SyntaxClassDto } from '@lingxi/bridge-client';

/**
 * Foreground color per semantic code class, for one theme.
 *
 * Diff segments cross the wire carrying BOTH a semantic `class` and the
 * terminal's resolved `rgb`. Only `class` is usable here: `rgb` was baked
 * against one dark terminal theme, so painting it on the light palette is
 * unreadable and it cannot follow the runtime theme toggle. Keying the color
 * off `class` is what makes a diff legible in both themes.
 */
export type SyntaxPalette = Readonly<Record<SyntaxClassDto, string>>;

const SYNTAX_DARK: SyntaxPalette = {
  plain: 'oklch(88% 0.010 270)',
  keyword: 'oklch(74% 0.150 320)',
  type_name: 'oklch(82% 0.120 85)',
  function: 'oklch(78% 0.120 245)',
  string_lit: 'oklch(76% 0.130 145)',
  number: 'oklch(80% 0.120 55)',
  comment: 'oklch(56% 0.025 270)',
  punctuation: 'oklch(68% 0.015 270)',
  operator: 'oklch(78% 0.090 200)',
  variable: 'oklch(85% 0.030 270)',
  constant: 'oklch(78% 0.130 30)',
  attribute: 'oklch(76% 0.110 300)',
};

const SYNTAX_LIGHT: SyntaxPalette = {
  plain: 'oklch(28% 0.020 270)',
  keyword: 'oklch(45% 0.190 320)',
  type_name: 'oklch(44% 0.130 60)',
  function: 'oklch(45% 0.170 250)',
  string_lit: 'oklch(42% 0.130 150)',
  number: 'oklch(47% 0.150 45)',
  comment: 'oklch(58% 0.030 270)',
  punctuation: 'oklch(46% 0.020 270)',
  operator: 'oklch(42% 0.110 200)',
  variable: 'oklch(33% 0.035 270)',
  constant: 'oklch(46% 0.170 25)',
  attribute: 'oklch(45% 0.150 300)',
};

export interface Tokens {
  /**
   * Whether this is the dark palette. Read by the diff renderer, which is
   * allowed to honor a segment's terminal `rgb` ONLY for `plain` runs in dark
   * mode — the one case where the terminal's baked color is on the right
   * background.
   */
  dark: boolean;
  appBg: string;
  windowBg: string;
  sidebarBg: string;
  stageBg: string;
  /** Layered background used only by the conversation transcript. */
  transcriptBg: string;
  surface: string;
  surfaceHover: string;
  surfaceActive: string;
  border: string;
  borderStrong: string;
  text: string;
  text2: string;
  text3: string;
  text4: string;
  accent: string;
  accentBg: string;
  accentBorder: string;
  accent2: string;
  accent3: string;
  ok: string;
  warn: string;
  danger: string;
  add: string;
  del: string;
  windowShadow: string;
  /** Optional link color — falls back to accent2/accent throughout the UI. */
  link?: string;
  /** Per-class code colors for diffs and syntax-highlighted bodies. */
  syntax: SyntaxPalette;
}

export const tokens = (dark: boolean): Tokens =>
  dark
    ? {
        dark: true,
        syntax: SYNTAX_DARK,
        appBg: '#0c0b10',
        windowBg: 'oklch(14% 0.012 270)',
        sidebarBg: 'oklch(12% 0.012 270)',
        stageBg: 'oklch(15% 0.012 270)',
        transcriptBg: 'radial-gradient(circle at 50% -12%, oklch(72% 0.18 268 / 0.065), transparent 46%), oklch(15% 0.012 270)',
        surface: 'oklch(18% 0.015 270)',
        surfaceHover: 'oklch(22% 0.020 270)',
        surfaceActive: 'oklch(26% 0.028 270)',
        border: 'oklch(26% 0.015 270 / 0.65)',
        borderStrong: 'oklch(34% 0.020 270 / 0.8)',
        text: 'oklch(96% 0.005 270)',
        text2: 'oklch(74% 0.015 270)',
        text3: 'oklch(54% 0.020 270)',
        text4: 'oklch(40% 0.020 270)',
        accent: 'oklch(72% 0.18 268)',
        accentBg: 'oklch(72% 0.18 268 / 0.14)',
        accentBorder: 'oklch(72% 0.18 268 / 0.35)',
        accent2: 'oklch(72% 0.18 320)',
        accent3: 'oklch(74% 0.16 195)',
        ok: 'oklch(72% 0.16 155)',
        warn: 'oklch(76% 0.15 75)',
        danger: 'oklch(68% 0.20 25)',
        add: 'oklch(72% 0.16 155)',
        del: 'oklch(68% 0.20 25)',
        windowShadow: '0 30px 80px rgba(0,0,0,0.55), 0 0 0 1px rgba(255,255,255,0.06)',
      }
    : {
        dark: false,
        syntax: SYNTAX_LIGHT,
        appBg: '#dcd7e4',
        windowBg: '#ffffff',
        sidebarBg: '#fcfcfc',
        stageBg: '#ffffff',
        transcriptBg: '#ffffff',
        surface: '#ffffff',
        surfaceHover: 'oklch(96% 0.008 270)',
        surfaceActive: 'oklch(92% 0.020 270)',
        border: 'oklch(88% 0.008 270)',
        borderStrong: 'oklch(82% 0.012 270)',
        text: 'oklch(20% 0.018 270)',
        text2: 'oklch(40% 0.020 270)',
        text3: 'oklch(58% 0.020 270)',
        text4: 'oklch(70% 0.015 270)',
        accent: 'oklch(50% 0.22 268)',
        accentBg: 'oklch(50% 0.22 268 / 0.10)',
        accentBorder: 'oklch(50% 0.22 268 / 0.30)',
        accent2: 'oklch(55% 0.22 320)',
        accent3: 'oklch(52% 0.18 195)',
        ok: 'oklch(50% 0.16 155)',
        warn: 'oklch(58% 0.16 65)',
        danger: 'oklch(56% 0.22 25)',
        add: 'oklch(50% 0.16 155)',
        del: 'oklch(56% 0.22 25)',
        windowShadow: '0 30px 80px rgba(60,40,120,0.18), 0 0 0 1px rgba(0,0,0,0.05)',
      };

export type ThemeMode = 'dark' | 'light';
