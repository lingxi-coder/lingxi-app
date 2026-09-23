import type { CSSProperties } from 'react';

export interface IconProps {
  name: string;
  size?: number;
  color?: string;
  stroke?: number;
  style?: CSSProperties;
}

export function Icon({ name, size = 16, color = 'currentColor', stroke = 1.6, style }: IconProps) {
  const p = {
    width: size,
    height: size,
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: color,
    strokeWidth: stroke,
    strokeLinecap: 'round' as const,
    strokeLinejoin: 'round' as const,
    style,
  };
  switch (name) {
    // Codex avatar from app-primary and node-to-node-path-light-16 from
    // open-sources-side-panel-tab-65eeed587bdb.js (2026-09-07).
    case 'subagent': return <img src={new URL('../assets/codex-subagent.svg', import.meta.url).href} width={size} height={size} alt="" style={style} />;
    case 'resources': return <svg width={size} height={size} viewBox="0 0 16 16" style={style} aria-hidden="true"><path fillRule="evenodd" clipRule="evenodd" d="M11.6665 2.14209C12.8764 2.14216 13.8569 3.12262 13.8569 4.33252C13.8569 5.54248 12.8764 6.52386 11.6665 6.52393C10.6383 6.52381 9.7775 5.81441 9.5415 4.85889H6.83447C6.18106 4.85889 5.87593 5.17088 5.79248 5.43115C5.71143 5.68496 5.78095 6.07453 6.27393 6.38525L10.2661 8.71436C10.2704 8.71684 10.2756 8.71955 10.2798 8.72217C11.1143 9.24411 11.4612 10.1035 11.2095 10.8901C10.9593 11.6711 10.18 12.1927 9.16748 12.1929H6.45654C6.22029 13.1478 5.36135 13.8569 4.3335 13.8569C3.1235 13.8569 2.14209 12.8755 2.14209 11.6655C2.14216 10.4556 3.12354 9.4751 4.3335 9.4751C5.36278 9.47515 6.22366 10.1853 6.4585 11.1421H9.16748C9.82062 11.142 10.126 10.83 10.2095 10.5698C10.2909 10.3151 10.2199 9.92412 9.72217 9.61279L5.73682 7.2876L5.72314 7.27979C4.8883 6.75802 4.54105 5.89856 4.79248 5.11182C5.04247 4.3305 5.8215 3.80909 6.83447 3.80908H9.54053C9.77551 2.85232 10.6373 2.1422 11.6665 2.14209ZM4.3335 10.5249C3.70344 10.5249 3.19294 11.0355 3.19287 11.6655C3.19287 12.2956 3.7034 12.8062 4.3335 12.8062C4.96354 12.8061 5.47412 12.2956 5.47412 11.6655C5.47406 11.0355 4.9635 10.525 4.3335 10.5249ZM11.6665 3.19189C11.0366 3.19203 10.5259 3.70256 10.5259 4.33252C10.5259 4.96254 11.0365 5.47301 11.6665 5.47314C12.2965 5.47308 12.8071 4.96258 12.8071 4.33252C12.8071 3.70252 12.2965 3.19196 11.6665 3.19189Z" fill={color}/></svg>;
    // Codex list-circle-light-16 — app-primary-6cd7b8b3f5e3.js (2026-09-07).
    case 'summary-list': return <svg width={size} height={size} viewBox="0 0 16 16" style={style} aria-hidden="true"><path fillRule="evenodd" clipRule="evenodd" d="M3.89062 8.81738C5.28506 8.81738 6.41588 9.94837 6.41602 11.3428C6.41602 12.7373 5.28514 13.8682 3.89062 13.8682C2.49611 13.8682 1.36523 12.7373 1.36523 11.3428C1.36537 9.94837 2.49619 8.81738 3.89062 8.81738ZM3.89062 9.86816C3.07609 9.86816 2.41615 10.5283 2.41602 11.3428C2.41602 12.1574 3.076 12.8174 3.89062 12.8174C4.70525 12.8174 5.36523 12.1574 5.36523 11.3428C5.3651 10.5283 4.70516 9.86816 3.89062 9.86816Z" fill={color}/> <path d="M14 10.8174C14.2898 10.8174 14.5253 11.053 14.5254 11.3428C14.5254 11.6327 14.2899 11.8681 14 11.8682H8.66699C8.37704 11.8682 8.1416 11.6327 8.1416 11.3428C8.14173 11.0529 8.37712 10.8174 8.66699 10.8174H14Z" fill={color}/> <path fillRule="evenodd" clipRule="evenodd" d="M3.89062 2.13965C5.28514 2.13965 6.41602 3.27052 6.41602 4.66504C6.41602 6.05956 5.28514 7.19043 3.89062 7.19043C2.49611 7.19043 1.36523 6.05956 1.36523 4.66504C1.36523 3.27052 2.49611 2.13965 3.89062 2.13965ZM3.89062 3.19043C3.076 3.19043 2.41602 3.85042 2.41602 4.66504C2.41602 5.47966 3.076 6.13965 3.89062 6.13965C4.70525 6.13965 5.36523 5.47966 5.36523 4.66504C5.36523 3.85042 4.70525 3.19043 3.89062 3.19043Z" fill={color}/> <path d="M14 4.13965C14.2899 4.13971 14.5254 4.37513 14.5254 4.66504C14.5254 4.95495 14.2899 5.19036 14 5.19043H8.66699C8.37704 5.19043 8.1416 4.95499 8.1416 4.66504C8.1416 4.37509 8.37704 4.13965 8.66699 4.13965H14Z" fill={color}/></svg>;
    case 'topbar-summary': return <svg {...p}><circle cx="6.5" cy="8" r="1.4" /><path d="M11 8h7M11 15h7" /><circle cx="6.5" cy="15" r="1.4" /></svg>;
    case 'topbar-terminal': return <svg {...p}><rect x="3.5" y="5.5" width="17" height="13" rx="3.25" /><path d="m7.25 10 2.5 2.5-2.5 2.5M12.5 15h4" /></svg>;
    case 'topbar-inspector-closed': return <svg {...p}><rect x="3.5" y="5.5" width="17" height="13" rx="3.25" /><path d="M15.5 6v12" /></svg>;
    case 'topbar-inspector-open': return <svg {...p}><rect x="15.8" y="6.2" width="4" height="11.6" rx="1.2" fill={color} fillOpacity=".14" stroke="none" /><rect x="3.5" y="5.5" width="17" height="13" rx="3.25" /><path d="M15.5 6v12" /></svg>;
    // Codex sidebar-light-16 — thread-panel-toggle-button-5a6d8096e132.js (2026-09-07).
    case 'panel-right': return <svg width={size} height={size} viewBox="0 0 16 16" style={style} aria-hidden="true"><g transform="translate(16 0) scale(-1 1)"><path fillRule="evenodd" clipRule="evenodd" d="M11.5 2.30762C13.1707 2.30762 14.5254 3.66235 14.5254 5.33301V10.666C14.5254 12.3367 13.1707 13.6914 11.5 13.6914H4.5C2.82934 13.6914 1.47461 12.3367 1.47461 10.666V5.33301C1.47461 3.66235 2.82934 2.30762 4.5 2.30762H11.5ZM6.52539 12.6416H11.5C12.5908 12.6416 13.4746 11.7568 13.4746 10.666V5.33301C13.4746 4.24225 12.5908 3.3584 11.5 3.3584H6.52539V12.6416ZM4.5 3.3584C3.40924 3.3584 2.52539 4.24225 2.52539 5.33301V10.666C2.52539 11.7568 3.40924 12.6416 4.5 12.6416H5.47461V3.3584H4.5Z" fill={color}/></g></svg>;
    // Codex sidebar-hidden-light-16 — thread-panel-toggle-button-5a6d8096e132.js (2026-09-07).
    case 'panel-right-hidden': return <svg width={size} height={size} viewBox="0 0 16 16" style={style} aria-hidden="true"><g transform="translate(16 0) scale(-1 1)"><path d="M4.66699 4.80859C4.95683 4.80873 5.19238 5.04412 5.19238 5.33398V10.667C5.19238 10.9569 4.95683 11.1923 4.66699 11.1924C4.37704 11.1924 4.1416 10.9569 4.1416 10.667V5.33398C4.1416 5.04403 4.37704 4.80859 4.66699 4.80859Z" fill={color}/> <path fillRule="evenodd" clipRule="evenodd" d="M11.5 2.30762C13.1707 2.30762 14.5254 3.66235 14.5254 5.33301V10.666C14.5254 12.3367 13.1707 13.6914 11.5 13.6914H4.5C2.82934 13.6914 1.47461 12.3367 1.47461 10.666V5.33301C1.47461 3.66235 2.82934 2.30762 4.5 2.30762H11.5ZM4.5 3.3584C3.40924 3.3584 2.52539 4.24225 2.52539 5.33301V10.666C2.52539 11.7568 3.40924 12.6416 4.5 12.6416H11.5C12.5908 12.6416 13.4746 11.7568 13.4746 10.666V5.33301C13.4746 4.24225 12.5908 3.3584 11.5 3.3584H4.5Z" fill={color}/></g></svg>;
    // Codex's thread-list error mark: a ringed glyph at the row's trailing edge.
    // NOT ported from a Codex bundle like the icons above — the Codex GUI is not
    // in this repo (its checkout here is the Rust CLI/TUI + SDK, whose only SVGs
    // belong to a vendored docs site) and the app is not installed, so there was
    // no `app-primary-*.js` to lift a path from. Drawn to match the reference
    // screenshot in the same 16px, `fill={color}` house style as those ports; if
    // the real bundle ever lands here, replace this with the genuine path.
    case 'circleAlert': return <svg width={size} height={size} viewBox="0 0 16 16" style={style} aria-hidden="true"><path fillRule="evenodd" clipRule="evenodd" d="M8 1.47461C11.6053 1.47461 14.5254 4.39472 14.5254 8C14.5254 11.6053 11.6053 14.5254 8 14.5254C4.39472 14.5254 1.47461 11.6053 1.47461 8C1.47461 4.39472 4.39472 1.47461 8 1.47461ZM8 2.52539C4.97487 2.52539 2.52539 4.97487 2.52539 8C2.52539 11.0251 4.97487 13.4746 8 13.4746C11.0251 13.4746 13.4746 11.0251 13.4746 8C13.4746 4.97487 11.0251 2.52539 8 2.52539Z" fill={color}/> <path d="M8 6.66699C8.28991 6.66706 8.52539 6.90251 8.52539 7.19238V11.1924C8.52539 11.4823 8.28991 11.7177 8 11.7178C7.71005 11.7178 7.47461 11.4823 7.47461 11.1924V7.19238C7.47461 6.90243 7.71005 6.66699 8 6.66699Z" fill={color}/> <path d="M8 4.28418C8.35304 4.28418 8.63932 4.57046 8.63932 4.9235C8.63932 5.27654 8.35304 5.56282 8 5.56282C7.64696 5.56282 7.36068 5.27654 7.36068 4.9235C7.36068 4.57046 7.64696 4.28418 8 4.28418Z" fill={color}/></svg>;
    case 'sidebar': return <svg {...p}><rect x="3" y="3" width="18" height="18" rx="2" /><path d="M9 3v18" /></svg>;
    case 'sidebarR': return <svg {...p}><rect x="3" y="3" width="18" height="18" rx="2" /><path d="M15 3v18" /></svg>;
    case 'search': return <svg {...p}><circle cx="11" cy="11" r="7" /><path d="m21 21-4.3-4.3" /></svg>;
    case 'plus': return <svg {...p}><path d="M12 5v14M5 12h14" /></svg>;
    case 'minus': return <svg {...p}><path d="M5 12h14" /></svg>;
    case 'check': return <svg {...p}><path d="M20 6 9 17l-5-5" /></svg>;
    case 'chevron': return <svg {...p}><path d="m6 9 6 6 6-6" /></svg>;
    case 'chevronR': return <svg {...p}><path d="m9 18 6-6-6-6" /></svg>;
    case 'arrowLeft': return <svg {...p}><path d="M19 12H5m7-7-7 7 7 7" /></svg>;
    case 'chevronL': return <svg {...p}><path d="m15 18-6-6 6-6" /></svg>;
    case 'code': return <svg {...p}><path d="m16 18 6-6-6-6M8 6l-6 6 6 6" /></svg>;
    case 'chat': return <svg {...p}><path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z" /></svg>;
    case 'chatPlus': return <svg {...p}><path d="M20 11.5a8 8 0 0 1-8.5 8 8.2 8.2 0 0 1-3.4-.8L3 20.5l1.7-5.1A8 8 0 1 1 20 11.5Z" /><path d="M9 11h6M12 8v6" /></svg>;
    case 'cowork': return <svg {...p}><path d="M9 6l-6 6 6 6M15 6l6 6-6 6" /><path d="M14 4l-4 16" /></svg>;
    case 'spark': return <svg {...p}><path d="M5 3v4M3 5h4M19 17v4M17 19h4M13 3l3 8 8 3-8 3-3 8-3-8-8-3 8-3z" /></svg>;
    case 'bolt': return <svg {...p}><path d="m13 2-10 12h7l-1 8 10-12h-7z" /></svg>;
    case 'box': return <svg {...p}><path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z" /><path d="m3.3 7 8.7 5 8.7-5M12 22V12" /></svg>;
    case 'more': return <svg {...p}><circle cx="12" cy="12" r="1" /><circle cx="19" cy="12" r="1" /><circle cx="5" cy="12" r="1" /></svg>;
    case 'mic': return <svg {...p}><rect x="9" y="2" width="6" height="11" rx="3" /><path d="M19 10v2a7 7 0 0 1-14 0v-2M12 19v3" /></svg>;
    case 'waveform': return <svg {...p}><path d="M4 10v4M8 7v10M12 4v16M16 7v10M20 10v4" /></svg>;
    case 'send': return <svg {...p}><path d="M22 2 11 13" /><path d="M22 2 15 22 11 13 2 9z" /></svg>;
    case 'stop': return <svg {...p}><rect x="6" y="6" width="12" height="12" rx="1.5" /></svg>;
    case 'sun': return <svg {...p}><circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" /></svg>;
    case 'moon': return <svg {...p}><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" /></svg>;
    case 'branch': return <svg {...p}><circle cx="6" cy="3" r="2" /><circle cx="6" cy="21" r="2" /><circle cx="18" cy="6" r="2" /><path d="M6 5v14M6 13a8 8 0 0 0 8 8M14 7h2a2 2 0 0 1 2 2v3" /></svg>;
    case 'git': return <svg {...p}><circle cx="12" cy="12" r="3" /><path d="M21 12h-6M9 12H3M12 9V3M12 21v-6" /></svg>;
    case 'file': return <svg {...p}><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" /><polyline points="14 2 14 8 20 8" /></svg>;
    case 'copy': return <svg {...p}><rect x="8" y="8" width="12" height="12" rx="2" /><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" /></svg>;
    case 'pin': return <svg {...p}><path d="m15 4 5 5-3 1-4 4 .5 4.5-1.5 1.5-3.5-5.5L4 11l1.5-1.5L10 10l4-4z" /><path d="m9 15-5 5" /></svg>;
    case 'image': return <svg {...p}><rect x="3" y="4" width="18" height="16" rx="2" /><circle cx="8.5" cy="9" r="1.5" /><path d="m3 16 5-5 4 4 2.5-2.5L21 18" /></svg>;
    case 'folder': return <svg {...p}><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /></svg>;
    case 'folderPlus': return <svg {...p}><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /><path d="M12 11v6M9 14h6" /></svg>;
    case 'terminal': return <svg {...p}><path d="m4 7 6 5-6 5M12 19h8" /></svg>;
    case 'hand': return <svg {...p}><path d="M6.5 11V7.5a1.5 1.5 0 0 1 3 0V10M9.5 10V5.5a1.5 1.5 0 0 1 3 0V10M12.5 10V6.5a1.5 1.5 0 0 1 3 0v4M15.5 10V8.5a1.5 1.5 0 0 1 3 0V14c0 4.4-2.8 7-7 7-3.2 0-5-1.5-6.4-4L3.3 13.8a1.6 1.6 0 0 1 2.7-1.7l1.5 2" /></svg>;
    case 'shieldCheck': return <svg {...p}><path d="M12 3 20 6v5c0 5.2-3.4 8.4-8 10-4.6-1.6-8-4.8-8-10V6l8-3Z" /><path d="m8.5 12 2.2 2.2 4.8-5" /></svg>;
    case 'shieldAlert': return <svg {...p}><path d="M12 3 20 6v5c0 5.2-3.4 8.4-8 10-4.6-1.6-8-4.8-8-10V6l8-3Z" /><path d="M12 8v5M12 16.5v.1" /></svg>;
    case 'lock': return <svg {...p}><rect x="5" y="10" width="14" height="11" rx="2" /><path d="M8 10V7a4 4 0 0 1 8 0v3M12 14v3" /></svg>;
    case 'cog': return <svg {...p}><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.9 2.9l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 0 1-4 0v-.1a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.9-2.9l.1-.1A1.7 1.7 0 0 0 4.6 15a1.7 1.7 0 0 0-1.5-1H3a2 2 0 0 1 0-4h.1A1.7 1.7 0 0 0 4.6 9a1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.9-2.9l.1.1A1.7 1.7 0 0 0 9 4.6a1.7 1.7 0 0 0 1-1.5V3a2 2 0 0 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.9 2.9l-.1.1A1.7 1.7 0 0 0 19.4 9 1.7 1.7 0 0 0 21 10H21a2 2 0 0 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" /></svg>;
    case 'arrowU': return <svg {...p} fill={color} stroke="none"><path d="M12 4l-7 8h4v8h6v-8h4z" /></svg>;
    case 'play': return <svg {...p} fill={color} stroke="none"><path d="M6 4l14 8-14 8z" /></svg>;
    case 'x': return <svg {...p}><path d="M18 6 6 18M6 6l12 12" /></svg>;
    case 'tasks': return <svg {...p}><path d="M12 2 4 9l8 7 8-7z" /><path d="m4 15 8 7 8-7" /></svg>;
    case 'dot': return <svg {...p}><circle cx="12" cy="12" r="3" fill={color} stroke="none" /></svg>;
    case 'circle': return <svg {...p}><circle cx="12" cy="12" r="6" /></svg>;
    case 'goal': return <svg {...p}><circle cx="12" cy="12" r="8" /><circle cx="12" cy="12" r="3" /><path d="M12 2v3M12 19v3M2 12h3M19 12h3" /></svg>;
    case 'archive': return <svg {...p}><rect x="3" y="3" width="18" height="5" rx="1" /><path d="M5 8v12h14V8M10 12h4" /></svg>;
    case 'pencil':
    case 'compose': return <svg {...p}><path d="M12 3H7a4 4 0 0 0-4 4v10a4 4 0 0 0 4 4h10a4 4 0 0 0 4-4v-5" /><path d="m10 14 1-4L18.5 2.5a2.12 2.12 0 0 1 3 3L14 13z" /></svg>;
    case 'bell': return <svg {...p}><path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9 0 1 18 1 18 0 0-2-3-2-3-9M10 21h4" /></svg>;
    case 'notebook': return <svg {...p}><rect x="6" y="3" width="14" height="18" rx="3" /><path d="M3 7h5M3 12h5M3 17h5M11 8h5M11 12h3" /></svg>;
    case 'fileSearch': return <svg {...p}><path d="M11 21H6a3 3 0 0 1-3-3V6a3 3 0 0 1 3-3h8l5 5v3M13 3v6h6" /><circle cx="16" cy="17" r="4" /><path d="m19 20 3 3" /></svg>;
    case 'sliders': return <svg {...p}><path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6" /></svg>;
    case 'clock': return <svg {...p}><circle cx="12" cy="12" r="9" /><path d="M12 7v5l3.5 2" /></svg>;
    case 'bulb': return <svg {...p}><path d="M9 18h6M10 22h4" /><path d="M8.3 15.2A7 7 0 1 1 15.7 15.2C14.7 16 14 16.8 14 18h-4c0-1.2-.7-2-1.7-2.8Z" /></svg>;
    case 'brain': return <svg {...p}><path d="M9.5 4.5A3 3 0 0 0 4 6.2a3 3 0 0 0 .4 5.6A3.5 3.5 0 0 0 9.5 18v-13.5ZM14.5 4.5A3 3 0 0 1 20 6.2a3 3 0 0 1-.4 5.6A3.5 3.5 0 0 1 14.5 18v-13.5Z" /><path d="M7 9.5h2.5M17 9.5h-2.5M7.5 15h2M16.5 15h-2" /></svg>;
    case 'refresh': return <svg {...p}><path d="M20 7v5h-5" /><path d="M4 17v-5h5" /><path d="M6.1 9a7 7 0 0 1 11.5-2.6L20 9M4 15l2.4 2.6A7 7 0 0 0 17.9 15" /></svg>;
    case 'hook': return <svg {...p}><path d="M15 5a3 3 0 1 0-6 0v11a5 5 0 0 0 10 0v-2" /><path d="m16 16 3-3 3 3" /></svg>;
    case 'user': return <svg {...p}><circle cx="12" cy="8" r="4" /><path d="M4.5 21a7.5 7.5 0 0 1 15 0" /></svg>;
    case 'users': return <svg {...p}><circle cx="9" cy="8" r="3.5" /><path d="M2.5 20a6.5 6.5 0 0 1 13 0M16 5.5a3.5 3.5 0 0 1 0 6.5M17 15a5.5 5.5 0 0 1 4.5 5" /></svg>;
    case 'logIn': return <svg {...p}><path d="M10 17l5-5-5-5M15 12H3" /><path d="M14 4h5a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-5" /></svg>;
    case 'logOut': return <svg {...p}><path d="m14 17 5-5-5-5M19 12H7" /><path d="M10 4H5a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h5" /></svg>;
    case 'trash': return <svg {...p}><path d="M4 7h16M9 7V4h6v3M6 7l1 14h10l1-14M10 11v6M14 11v6" /></svg>;
    case 'share': return <svg {...p}><path d="M12 3v13M7 8l5-5 5 5" /><path d="M5 13v6a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-6" /></svg>;
    // The settings nav (`nav.ts`) references these ten by name; added here so
    // every page gets a real glyph instead of silently rendering nothing.
    case 'key': return <svg {...p}><circle cx="8" cy="15" r="4" /><path d="m10.5 12.5 8-8M16 5l3 3M13 8l3 3" /></svg>;
    case 'plug': return <svg {...p}><path d="M9 2v6M15 2v6M7 8h10l-1 5a5 5 0 0 1-5 4 5 5 0 0 1-5-4z" /><path d="M12 19v3" /></svg>;
    case 'shield': return <svg {...p}><path d="M12 3 20 6v5c0 5.2-3.4 8.4-8 10-4.6-1.6-8-4.8-8-10V6l8-3Z" /></svg>;
    case 'sparkle': return <svg {...p}><path d="M12 3v4M12 17v4M3 12h4M17 12h4M6 6l2.5 2.5M15.5 15.5 18 18M18 6l-2.5 2.5M8.5 15.5 6 18" /></svg>;
    case 'server': return <svg {...p}><rect x="3" y="4" width="18" height="7" rx="1.5" /><rect x="3" y="13" width="18" height="7" rx="1.5" /><path d="M7 7.5h.01M7 16.5h.01" /></svg>;
    case 'mcp': return <svg {...p}><path d="m10.5 13.5 5.7-5.7a3 3 0 1 0-4.2-4.2L5.2 10.4a5 5 0 0 0 7.1 7.1l6.1-6.1a2 2 0 1 0-2.8-2.8l-6.2 6.2" /><path d="m7.3 12.7 5-5" /></svg>;
    case 'anchor': return <svg {...p}><circle cx="12" cy="5" r="2.5" /><path d="M12 7.5V21M6 12H3a9 9 0 0 0 9 9 9 9 0 0 0 9-9h-3" /></svg>;
    case 'puzzle': return <svg {...p}><path d="M9 3h4a1 1 0 0 1 1 1v2.2a1.8 1.8 0 1 0 0 3.6V12a1 1 0 0 1-1 1h-2.2a1.8 1.8 0 1 0-3.6 0H5a1 1 0 0 1-1-1V9a1 1 0 0 1 1-1h2.2a1.8 1.8 0 1 0 0-3.6V4a1 1 0 0 1 1-1z" /></svg>;
    case 'braces': return <svg {...p}><path d="M8 3C6 3 6 5 6 7s0 3-2 4c2 1 2 2 2 4s0 4 2 4M16 3c2 0 2 2 2 4s0 3 2 4c-2 1-2 2-2 4s0 4-2 4" /></svg>;
    case 'activity': return <svg {...p}><path d="M22 12h-4l-3 8-6-16-3 8H2" /></svg>;
    case 'gauge': return <svg {...p}><path d="M4.9 19a9 9 0 1 1 14.2 0" /><path d="m12 14 4-4" /><path d="M12 19h.01" /></svg>;
    case 'compact': return <svg {...p}><path d="M7 3h7l4 4v8a4 4 0 0 1-4 4H7a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Z" /><path d="M14 3v5h5M8 11h6M8 15h3" /><circle cx="16.5" cy="17.5" r="2.5" /></svg>;
    case 'summary': return <svg {...p}><rect x="4" y="3" width="16" height="18" rx="3" /><path d="M8 8h8M8 12h8M8 16h5" /></svg>;
    case 'info': return <svg {...p}><circle cx="12" cy="12" r="9" /><path d="M12 11v6M12 7.5v.01" /></svg>;
    default: return null;
  }
}
