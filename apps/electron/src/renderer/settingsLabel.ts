const labels: Readonly<Record<string, string>> = {
  en: 'Settings',
  ja: '設定',
  ko: '설정',
  fr: 'Paramètres',
  de: 'Einstellungen',
  es: 'Configuración',
  pt: 'Configurações',
  it: 'Impostazioni',
  ru: 'Настройки',
  ar: 'الإعدادات',
};

/** Settings entry follows the system locale until desktop UI locale selection exists. */
export function settingsLabel(locale = typeof navigator === 'undefined' ? 'en' : navigator.language): string {
  const parts = locale.replace(/_/g, '-').toLowerCase().split('-');
  if (parts[0] === 'zh') {
    const traditional = parts.includes('hant')
      || (!parts.includes('hans') && parts.some((part) => ['tw', 'hk', 'mo'].includes(part)));
    return traditional ? '設定' : '设置';
  }
  return labels[parts[0] ?? 'en'] ?? labels.en!;
}
