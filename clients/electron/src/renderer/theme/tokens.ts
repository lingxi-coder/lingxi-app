// ─── DESIGN TOKENS — Lingxi Code dark/light ─────────────────
// Ported verbatim from the design prototype's `tokens(dark)` factory.
// Colors are CSS oklch(...) values and are kept exactly as-is.

export interface Tokens {
  appBg: string;
  windowBg: string;
  sidebarBg: string;
  stageBg: string;
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
}

export const tokens = (dark: boolean): Tokens =>
  dark
    ? {
        appBg: '#0c0b10',
        windowBg: 'oklch(14% 0.012 270)',
        sidebarBg: 'oklch(12% 0.012 270)',
        stageBg: 'oklch(15% 0.012 270)',
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
        appBg: '#dcd7e4',
        windowBg: '#fbf9f5',
        sidebarBg: '#f4f1ec',
        stageBg: '#fbf9f5',
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
