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
    case 'sidebar': return <svg {...p}><rect x="3" y="3" width="18" height="18" rx="2" /><path d="M9 3v18" /></svg>;
    case 'sidebarR': return <svg {...p}><rect x="3" y="3" width="18" height="18" rx="2" /><path d="M15 3v18" /></svg>;
    case 'search': return <svg {...p}><circle cx="11" cy="11" r="7" /><path d="m21 21-4.3-4.3" /></svg>;
    case 'plus': return <svg {...p}><path d="M12 5v14M5 12h14" /></svg>;
    case 'minus': return <svg {...p}><path d="M5 12h14" /></svg>;
    case 'check': return <svg {...p}><path d="M20 6 9 17l-5-5" /></svg>;
    case 'chevron': return <svg {...p}><path d="m6 9 6 6 6-6" /></svg>;
    case 'chevronR': return <svg {...p}><path d="m9 18 6-6-6-6" /></svg>;
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
    case 'compose': return <svg {...p}><path d="M12 3H7a4 4 0 0 0-4 4v10a4 4 0 0 0 4 4h10a4 4 0 0 0 4-4v-5" /><path d="m10 14 1-4L18.5 2.5a2.12 2.12 0 0 1 3 3L14 13z" /></svg>;
    case 'bell': return <svg {...p}><path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9 0 1 18 1 18 0 0-2-3-2-3-9M10 21h4" /></svg>;
    case 'notebook': return <svg {...p}><rect x="6" y="3" width="14" height="18" rx="3" /><path d="M3 7h5M3 12h5M3 17h5M11 8h5M11 12h3" /></svg>;
    case 'fileSearch': return <svg {...p}><path d="M11 21H6a3 3 0 0 1-3-3V6a3 3 0 0 1 3-3h8l5 5v3M13 3v6h6" /><circle cx="16" cy="17" r="4" /><path d="m19 20 3 3" /></svg>;
    case 'pencil': return <svg {...p}><path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z" /></svg>;
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
