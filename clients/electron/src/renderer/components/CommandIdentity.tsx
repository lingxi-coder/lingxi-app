import type { CSSProperties } from 'react';
import { useT } from '../theme/ThemeContext';
import { commandPaletteColor, commandPaletteIcon, commandPaletteName } from './commandPaletteIcons';
import { Icon } from './Icon';

/** The same command keeps its identity in the editor, menu, transcript, and results. */
export function CommandIcon({ command, size = 28 }: { command: string; size?: number }) {
  const t = useT();
  const icon = commandPaletteIcon(command);
  const color = commandPaletteColor(command, t.dark);
  return (
    <span className="command-identity-icon" data-command-icon={icon} aria-hidden="true" style={{
      '--command-identity-color': color,
      '--command-identity-surface': t.surface,
      '--command-icon-size': `${size}px`,
      color,
    } as CSSProperties}>
      <Icon name={icon} size={size >= 28 ? 17 : 14} stroke={1.75} />
    </span>
  );
}

export function CommandIdentity({ command, showSlash = false, iconSize = 24 }: {
  command: string;
  showSlash?: boolean;
  iconSize?: number;
}) {
  const t = useT();
  const name = commandPaletteName(command);
  return (
    <span className="command-identity" data-command-name={name} title={`/${name}`} style={{
      '--command-identity-color': commandPaletteColor(command, t.dark),
    } as CSSProperties}>
      <CommandIcon command={command} size={iconSize} />
      <span className="command-identity-label">{showSlash ? `/${name}` : name}</span>
    </span>
  );
}
