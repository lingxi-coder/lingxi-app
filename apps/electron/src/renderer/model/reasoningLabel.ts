import type { ReasoningSelectionDto } from '@lingxi/bridge-client';
import { currentLocale, translate, type DesktopLocale } from '../i18n/locale';

/** Translate display names without changing the provider's reasoning selection. */
export function reasoningSelectionLabel(selection?: ReasoningSelectionDto, locale: DesktopLocale = currentLocale()): string {
  const text = (key: string, ...args: readonly (string | number)[]) => translate(key, locale, ...args);
  if (!selection) return text('composer_effort_auto');
  switch (selection.type) {
    case 'automatic': return text('composer_effort_auto');
    case 'disabled': return text('composer_effort_off');
    case 'enabled': return text('composer_effort_on');
    case 'level': {
      const labels: Record<string, string> = {
        minimal: 'composer_effort_minimal', low: 'composer_effort_low', medium: 'composer_effort_medium',
        high: 'composer_effort_high', xhigh: 'composer_effort_xhigh', extra_high: 'composer_effort_xhigh', max: 'composer_effort_max',
      };
      return labels[selection.id] ? text(labels[selection.id]!) : selection.id.replace(/[-_]/g, ' ').replace(/\b\w/g, letter => letter.toUpperCase());
    }
    case 'token_budget': return text('desktop_chat_reasoning_tokens', selection.tokens.toLocaleString(locale));
    default: return text('composer_effort_auto');
  }
}
