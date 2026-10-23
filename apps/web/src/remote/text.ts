// Characters that can reorder or hide surrounding text: the bidirectional embeddings, overrides and
// isolates (U+202A-202E, U+2066-2069), the directional marks (U+200E, U+200F, U+061C), and the
// invisible separators that make two names look alike (zero-width space, word joiner, BOM).
const BIDI_AND_INVISIBLE = /[\u061C\u200B\u200E\u200F\u202A-\u202E\u2060\u2066-\u2069\uFEFF]/gu;
// C0 and C1 controls, DEL and the Unicode line and paragraph separators.
const CONTROLS = /[\u0000-\u001F\u007F-\u009F\u2028\u2029]/gu;

/**
 * A name or label that came from another device or from the desktop, made safe
 * to show on one line: direction controls and invisible characters removed,
 * control characters and line breaks turned into spaces, runs of spaces folded,
 * and the result cut to `maxChars` characters (by code point, never mid-pair).
 */
export function displayText(value: unknown, maxChars = 80): string {
  if (typeof value !== 'string') return '';
  const clean = value.replace(BIDI_AND_INVISIBLE, '').replace(CONTROLS, ' ').replace(/\s+/gu, ' ').trim();
  const points = Array.from(clean);
  return points.length <= maxChars ? clean : `${points.slice(0, Math.max(0, maxChars - 1)).join('').trimEnd()}…`;
}

/** The longest device name the viewer sends. The offer record allows 128, the plan asks for 64. */
export const DEVICE_NAME_MAX_CHARS = 64;

/**
 * "Chrome · macOS": the browser and operating system named in a user-agent
 * string. This is what the desktop shows next to the pairing code, so it is
 * deliberately coarse; the user recognizes their own browser, not a version.
 * `platformHint` is `navigator.userAgentData.platform` where the browser has it.
 */
export function deviceNameFromUserAgent(userAgent: string, platformHint?: string): string {
  const browser = browserName(userAgent);
  const system = systemName(userAgent, platformHint);
  return displayText(`${browser} · ${system}`, DEVICE_NAME_MAX_CHARS);
}

function browserName(ua: string): string {
  // Order matters: Edge, Opera and Samsung Internet all say "Chrome", Chrome says "Safari".
  if (/\bEdg(?:e|A|iOS)?\//u.test(ua)) return 'Edge';
  if (/\bOPR\/|\bOpera\b/u.test(ua)) return 'Opera';
  if (/\bSamsungBrowser\//u.test(ua)) return 'Samsung Internet';
  if (/\bFirefox\/|\bFxiOS\//u.test(ua)) return 'Firefox';
  if (/\bChrome\/|\bCriOS\//u.test(ua)) return 'Chrome';
  if (/\bSafari\//u.test(ua)) return 'Safari';
  return 'Browser';
}

function systemName(ua: string, platformHint?: string): string {
  if (/\b(?:iPhone|iPod)\b/u.test(ua)) return 'iOS';
  if (/\biPad\b/u.test(ua)) return 'iPadOS';
  if (/\bAndroid\b/u.test(ua)) return 'Android';
  if (/\bCrOS\b/u.test(ua)) return 'ChromeOS';
  if (/\bWindows\b/u.test(ua)) return 'Windows';
  if (/\bMac(?:intosh| OS X)\b/u.test(ua)) return 'macOS';
  if (/\bLinux\b|\bX11\b/u.test(ua)) return 'Linux';
  const hint = displayText(platformHint ?? '', 16);
  return hint || 'Web';
}

/** Group a pairing code in threes so two people can compare it by eye: `"123456"` becomes `"123 456"`. */
export function formatPairingCode(code: string): string {
  return /^\d{6}$/u.test(code) ? `${code.slice(0, 3)} ${code.slice(3)}` : code;
}
