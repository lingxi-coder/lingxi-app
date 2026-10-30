import React, { type ReactNode } from 'react';
import { Icon, type IconProps } from './Icon';

// Original, SF Symbols-inspired drawings for the desktop navigation chrome.
// A shared optical frame and regular weight keep small toolbar symbols balanced.
const symbols: Record<string, ReactNode> = {
  plus: <path d="M12 4.5v15M4.5 12h15" />,
  expand: <path d="M14 4h6v6M20 4l-6.5 6.5M10 20H4v-6M4 20l6.5-6.5" />,
  collapse: <path d="M20 10h-6V4M14 10l6-6M4 14h6v6M10 14l-6 6" />,
  sidebar: <React.Fragment><rect x="2.75" y="4.5" width="18.5" height="15" rx="3" /><path d="M9 4.75v14.5" /></React.Fragment>,
  home: <React.Fragment><path d="m3 10 8-6.5a1.6 1.6 0 0 1 2 0l8 6.5M5.25 8.5v10a1.75 1.75 0 0 0 1.75 1.75h3v-6h4v6h3a1.75 1.75 0 0 0 1.75-1.75v-10" /></React.Fragment>,
  apps: <React.Fragment>{[4, 13.5].flatMap((x) => [4, 13.5].map((y) => <rect key={`${x}-${y}`} x={x} y={y} width="6.5" height="6.5" rx="1.9" />))}</React.Fragment>,
  search: <React.Fragment><circle cx="10.5" cy="10.5" r="6.5" /><path d="m15.25 15.25 5 5" /></React.Fragment>,
  bell: <React.Fragment><path d="M5 17.25h14c-1.5-1.75-2-3-2-7a5 5 0 0 0-10 0c0 4-.5 5.25-2 7ZM10 20a2.5 2.5 0 0 0 4 0M12 3.25V2" /></React.Fragment>,
  clock: <React.Fragment><circle cx="12" cy="12" r="8.5" /><path d="M12 6.75v5.5l3.5 2" /></React.Fragment>,
  cog: <React.Fragment><path d="m10 3-.5 2-1.75 1L5.7 5.5l-2 3.5 1.5 1.5v3L3.7 15l2 3.5 2.05-.5 1.75 1 .5 2h4l.5-2 1.75-1 2.05.5 2-3.5-1.5-1.5v-3L20.3 9l-2-3.5-2.05.5-1.75-1-.5-2Z" /><circle cx="12" cy="12" r="3.25" /></React.Fragment>,
  branch: <React.Fragment><circle cx="7" cy="5" r="2" /><circle cx="7" cy="19" r="2" /><circle cx="17" cy="5" r="2" /><path d="M7 7v10M17 7v1a5 5 0 0 1-5 5H7" /></React.Fragment>,
  folder: <path d="M3 8V6.5a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8Zm0 1h18" />,
  compose: <React.Fragment><path d="M11 4.5H6a2.5 2.5 0 0 0-2.5 2.5v11A2.5 2.5 0 0 0 6 20.5h11a2.5 2.5 0 0 0 2.5-2.5v-5" /><path d="m10 14 .75-3.75L18.5 2.5a1.77 1.77 0 0 1 2.5 2.5l-7.75 7.75L10 14Zm6.75-9.75 2.5 2.5" /></React.Fragment>,
  chat: <path d="M7 4.5h10A3.5 3.5 0 0 1 20.5 8v6a3.5 3.5 0 0 1-3.5 3.5h-5.75L6 21v-3.65A3.5 3.5 0 0 1 3.5 14V8A3.5 3.5 0 0 1 7 4.5Z" />,
  'topbar-summary': <React.Fragment><path d="M9.5 6.5h10M9.5 12h10M9.5 17.5h10" /><path d="M4.5 6.5h.01M4.5 12h.01M4.5 17.5h.01" strokeWidth="2.8" /></React.Fragment>,
  'topbar-terminal': <React.Fragment><rect x="2.75" y="4.5" width="18.5" height="15" rx="3" /><path d="m6.75 9 3 3-3 3M13 15h4.25" /></React.Fragment>,
  'topbar-inspector-closed': <React.Fragment><rect x="2.75" y="4.5" width="18.5" height="15" rx="3" /><path d="M15 4.75v14.5" /></React.Fragment>,
  'topbar-inspector-open': <React.Fragment><rect x="15" y="4.5" width="6.25" height="15" rx="2" fill="currentColor" fillOpacity=".18" stroke="none" /><rect x="2.75" y="4.5" width="18.5" height="15" rx="3" /><path d="M15 4.75v14.5" /></React.Fragment>,
};

export function DesktopIcon({ name, size = 20, color = 'currentColor', style }: IconProps) {
  const symbol = symbols[name];
  if (!symbol) return <Icon name={name} size={size} color={color} style={style} />;
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color}
    strokeWidth="1.65" strokeLinecap="round" strokeLinejoin="round" style={style}
    aria-hidden="true" focusable="false">{symbol}</svg>;
}
