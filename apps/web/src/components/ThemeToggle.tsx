import { IconMoon, IconSun } from './Icons';

interface ThemeToggleProps {
  theme: 'light' | 'dark';
  locale: 'en' | 'zh';
  onToggle: () => void;
}

export function ThemeToggle({ theme, onToggle, locale }: ThemeToggleProps) {
  return (
    <button className="control-chip" type="button" onClick={onToggle} aria-label={locale === 'zh' ? (theme === 'light' ? '切换为深色主题' : '切换为浅色主题') : (theme === 'light' ? 'Switch to dark theme' : 'Switch to light theme')} aria-pressed={theme === 'dark'}>
      {theme === 'light' ? <IconMoon width={16} height={16} /> : <IconSun width={16} height={16} />}
      <span>{locale === 'zh' ? (theme === 'light' ? '深色' : '浅色') : (theme === 'light' ? 'Dark' : 'Light')}</span>
    </button>
  );
}
