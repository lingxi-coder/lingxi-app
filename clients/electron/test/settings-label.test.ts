import assert from 'node:assert/strict';
import test from 'node:test';
import { settingsLabel } from '../src/renderer/settingsLabel';

test('settings entry uses the system language label with English fallback', () => {
  for (const [locale, expected] of Object.entries({
    'en-US': 'Settings', 'zh-CN': '设置', 'zh_TW': '設定', 'zh-HK': '設定',
    'zh-Hans-HK': '设置', 'zh-Hant': '設定', 'ja-JP': '設定', 'ko-KR': '설정',
    'fr-FR': 'Paramètres', 'de-DE': 'Einstellungen', 'es-ES': 'Configuración',
    'pt-BR': 'Configurações', 'it-IT': 'Impostazioni', 'ru-RU': 'Настройки',
    'ar': 'الإعدادات', 'unknown': 'Settings',
  })) assert.equal(settingsLabel(locale), expected, locale);
});
