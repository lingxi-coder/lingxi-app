export type Locale = 'en' | 'zh';
export type Region = 'global' | 'china';

export interface LocalizedText {
  en: string;
  zh: string;
}

export function pickLocaleText(locale: Locale, text: LocalizedText): string {
  return locale === 'zh' ? text.zh : text.en;
}

export function detectInitialRegion(): Region {
  const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  return timezone.includes('Shanghai') || timezone.includes('Hong_Kong') ? 'china' : 'global';
}

export function formatCurrency(amountUsd: number, region: Region): string {
  const sign = amountUsd < 0 ? '-' : '';
  const absoluteAmount = Math.abs(amountUsd);
  if (region === 'china') {
    return `${sign}CN¥${Math.round(absoluteAmount * 7.15)}`;
  }
  return `${sign}$${absoluteAmount.toFixed(absoluteAmount >= 100 ? 0 : 2)}`;
}
