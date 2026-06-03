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
    case 'cowork': return <svg {...p}><path d="M9 6l-6 6 6 6M15 6l6 6-6 6" /><path d="M14 4l-4 16" /></svg>;
    case 'spark': return <svg {...p}><path d="M5 3v4M3 5h4M19 17v4M17 19h4M13 3l3 8 8 3-8 3-3 8-3-8-8-3 8-3z" /></svg>;
    case 'bolt': return <svg {...p}><path d="m13 2-10 12h7l-1 8 10-12h-7z" /></svg>;
    case 'box': return <svg {...p}><path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z" /><path d="m3.3 7 8.7 5 8.7-5M12 22V12" /></svg>;
    case 'more': return <svg {...p}><circle cx="12" cy="12" r="1" /><circle cx="19" cy="12" r="1" /><circle cx="5" cy="12" r="1" /></svg>;
    case 'mic': return <svg {...p}><rect x="9" y="2" width="6" height="11" rx="3" /><path d="M19 10v2a7 7 0 0 1-14 0v-2M12 19v3" /></svg>;
    case 'send': return <svg {...p}><path d="M22 2 11 13" /><path d="M22 2 15 22 11 13 2 9z" /></svg>;
    case 'stop': return <svg {...p}><rect x="6" y="6" width="12" height="12" rx="1.5" /></svg>;
    case 'sun': return <svg {...p}><circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" /></svg>;
    case 'moon': return <svg {...p}><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" /></svg>;
    case 'branch': return <svg {...p}><circle cx="6" cy="3" r="2" /><circle cx="6" cy="21" r="2" /><circle cx="18" cy="6" r="2" /><path d="M6 5v14M6 13a8 8 0 0 0 8 8M14 7h2a2 2 0 0 1 2 2v3" /></svg>;
    case 'git': return <svg {...p}><circle cx="12" cy="12" r="3" /><path d="M21 12h-6M9 12H3M12 9V3M12 21v-6" /></svg>;
    case 'file': return <svg {...p}><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" /><polyline points="14 2 14 8 20 8" /></svg>;
    case 'folder': return <svg {...p}><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /></svg>;
    case 'terminal': return <svg {...p}><path d="m4 7 6 5-6 5M12 19h8" /></svg>;
    case 'cog': return <svg {...p}><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.9 2.9l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 0 1-4 0v-.1a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.9-2.9l.1-.1A1.7 1.7 0 0 0 4.6 15a1.7 1.7 0 0 0-1.5-1H3a2 2 0 0 1 0-4h.1A1.7 1.7 0 0 0 4.6 9a1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.9-2.9l.1.1A1.7 1.7 0 0 0 9 4.6a1.7 1.7 0 0 0 1-1.5V3a2 2 0 0 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.9 2.9l-.1.1A1.7 1.7 0 0 0 19.4 9 1.7 1.7 0 0 0 21 10H21a2 2 0 0 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" /></svg>;
    case 'arrowU': return <svg {...p} fill={color} stroke="none"><path d="M12 4l-7 8h4v8h6v-8h4z" /></svg>;
    case 'play': return <svg {...p} fill={color} stroke="none"><path d="M6 4l14 8-14 8z" /></svg>;
    case 'x': return <svg {...p}><path d="M18 6 6 18M6 6l12 12" /></svg>;
    case 'tasks': return <svg {...p}><path d="M12 2 4 9l8 7 8-7z" /><path d="m4 15 8 7 8-7" /></svg>;
    case 'dot': return <svg {...p}><circle cx="12" cy="12" r="3" fill={color} stroke="none" /></svg>;
    case 'circle': return <svg {...p}><circle cx="12" cy="12" r="6" /></svg>;
    case 'pencil': return <svg {...p}><path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z" /></svg>;
    case 'sliders': return <svg {...p}><path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6" /></svg>;
    default: return null;
  }
}
