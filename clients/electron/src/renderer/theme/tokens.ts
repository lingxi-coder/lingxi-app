// ─── DESIGN TOKENS — Lingxi Code dark/light ─────────────────
// Shared renderer palette: neutral macOS surfaces, clear semantic labels,
// and a consistent system-blue accent in both appearances.

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
        appBg: '#1c1c1e',
        windowBg: '#1e1e20',
        sidebarBg: '#272729',
        stageBg: '#1e1e20',
        transcriptBg: '#1e1e20',
        surface: '#2c2c2e',
        surfaceHover: '#363638',
        surfaceActive: '#414144',
        border: 'rgba(255,255,255,0.09)',
        borderStrong: 'rgba(255,255,255,0.18)',
        text: '#f5f5f7',
        text2: '#c2c2c7',
        text3: '#a1a1a6',
        text4: '#919196',
        accent: '#0a84ff',
        accentBg: 'rgba(10,132,255,0.18)',
        accentBorder: 'rgba(10,132,255,0.38)',
        accent2: '#64aaff',
        accent3: '#64d2ff',
        link: '#64aaff',
        ok: '#32d74b',
        warn: '#ffd60a',
        danger: '#ff6961',
        add: '#32d74b',
        del: '#ff6961',
        windowShadow: '0 24px 64px rgba(0,0,0,0.38), 0 0 0 1px rgba(255,255,255,0.08)',
      }
    : {
        dark: false,
        syntax: SYNTAX_LIGHT,
        appBg: '#e5e5e7',
        windowBg: '#ffffff',
        sidebarBg: '#f2f2f4',
        stageBg: '#ffffff',
        transcriptBg: '#ffffff',
        surface: '#ffffff',
        surfaceHover: '#f2f2f4',
        surfaceActive: '#e5e5ea',
        border: 'rgba(60,60,67,0.10)',
        borderStrong: 'rgba(60,60,67,0.22)',
        text: '#1d1d1f',
        text2: '#515154',
        text3: '#636366',
        text4: '#6e6e73',
        // A deeper system blue keeps small labels legible on light surfaces.
        accent: '#0066d6',
        accentBg: 'rgba(0,102,214,0.10)',
        accentBorder: 'rgba(0,102,214,0.24)',
        accent2: '#0066d6',
        accent3: '#007a9f',
        link: '#0066d6',
        ok: '#248a3d',
        warn: '#996500',
        danger: '#c9342c',
        add: '#248a3d',
        del: '#c9342c',
        windowShadow: '0 24px 64px rgba(0,0,0,0.14), 0 0 0 1px rgba(0,0,0,0.06)',
      };

export type ThemeMode = 'dark' | 'light';

/**
 * The persisted preference, which is one entry wider than `ThemeMode`:
 * `'system'` is not a third palette (there are only two — see `tokens`
 * above) but a request to resolve to whichever of the two matches the OS at
 * render time, and to keep following the OS while mounted.
 */
export type ThemePreference = ThemeMode | 'system';

/** Resolves a preference to an actual palette, given the OS's current pick. */
export function resolveThemeMode(preference: ThemePreference | undefined, prefersDark: boolean): ThemeMode {
  if (preference === 'dark' || preference === 'light') return preference;
  return prefersDark ? 'dark' : 'light';
}

/**
 * The slice of `MediaQueryList` `watchThemePreference` needs — narrow enough
 * to fake in a unit test without a real DOM.
 */
export interface SystemColorSchemeQuery {
  readonly matches: boolean;
  addEventListener(type: 'change', listener: () => void): void;
  removeEventListener(type: 'change', listener: () => void): void;
}

/**
 * Resolves `preference` to a `ThemeMode` and reports it via `onChange`. For
 * `'system'` — and for an ABSENT preference, which means the same thing —
 * this also subscribes to the OS query's `change` event, so a user flipping
 * their OS appearance while the app is open is followed live; resolving once
 * at mount and never again would pass a cursory look but silently stop
 * tracking the OS. Returns a cleanup function that removes any listener it
 * attached (a no-op for an explicit `'dark'`/`'light'`).
 *
 * An absent preference is `'system'`, not "do nothing". `Appearance.tsx`
 * renders `settings.theme ?? 'system'` as the selected pill, and
 * `resolveThemeMode` already reads `undefined` as "ask the OS" — an early
 * return here was the one place in that chain that disagreed, and it
 * disagreed silently. On a fresh install with no persisted preference, the
 * app kept `App.tsx`'s `useState<ThemeMode>('dark')` seed no matter what the
 * OS said, while Settings → 外观 highlighted 跟随系统 and flipping the OS
 * theme changed nothing; clicking the already-selected 跟随系统 pill was the
 * only way out, and it fixed it permanently, which is the signature of a
 * default that was never resolved rather than of a preference.
 */
export function watchThemePreference(
  preference: ThemePreference | undefined,
  query: SystemColorSchemeQuery,
  onChange: (mode: ThemeMode) => void,
): () => void {
  if (preference === 'dark' || preference === 'light') {
    onChange(preference);
    return () => {};
  }
  const applySystemTheme = () => onChange(resolveThemeMode(preference, query.matches));
  applySystemTheme();
  query.addEventListener('change', applySystemTheme);
  return () => query.removeEventListener('change', applySystemTheme);
}
