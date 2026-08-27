import { IconMoon, IconSun } from './Icons';

interface ThemeToggleProps {
  theme: 'light' | 'dark';
  onToggle: () => void;
}

export function ThemeToggle({ theme, onToggle }: ThemeToggleProps) {
  return (
    <button className="control-chip" type="button" onClick={onToggle} aria-label="Toggle theme">
      {theme === 'light' ? <IconMoon width={16} height={16} /> : <IconSun width={16} height={16} />}
      <span>{theme === 'light' ? 'Dark' : 'Light'}</span>
    </button>
  );
}
