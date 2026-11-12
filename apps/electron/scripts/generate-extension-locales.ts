/** Generate the renderer's locale-specific chunks from the editable catalogs. */
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { decisionCatalog } from '../src/renderer/i18n/decisionCatalog';
import { localAppWorkspaceCatalog } from '../src/renderer/i18n/localAppWorkspaceCatalog';
import { providerAccountsCatalog } from '../src/renderer/i18n/providerAccountsCatalog';
import { botsCatalog } from '../src/renderer/i18n/botsCatalog';
import { chatCatalog } from '../src/renderer/i18n/chatCatalog';
import { computerCatalog } from '../src/renderer/i18n/computerCatalog';
import { desktopCatalog } from '../src/renderer/i18n/desktopCatalog';
import { extensionCatalog } from '../src/renderer/i18n/extensionCatalog';
import { flowCatalog } from '../src/renderer/i18n/flowCatalog';
import { gitCatalog } from '../src/renderer/i18n/gitCatalog';
import { imageCatalog } from '../src/renderer/i18n/imageCatalog';
import { interactionCatalog } from '../src/renderer/i18n/interactionCatalog';
import { miscCatalog } from '../src/renderer/i18n/miscCatalog';
import { navigationCatalog } from '../src/renderer/i18n/navigationCatalog';
import { runtimeCatalog } from '../src/renderer/i18n/runtimeCatalog';
import { settingsCatalog } from '../src/renderer/i18n/settingsCatalog';
import { shellCatalog } from '../src/renderer/i18n/shellCatalog';
import { uiCatalog } from '../src/renderer/i18n/uiCatalog';
import { voiceSettingsCatalog } from '../src/renderer/i18n/voiceSettingsCatalog';
import { updateCatalog } from '../src/renderer/i18n/updateCatalog';

export const extensionLocales = ['zh-Hans', 'zh-Hant', 'en', 'ja', 'ko'] as const;
// Preserve the original whole-entry override order, including absent languages.
export const extensionCatalogs = { ...decisionCatalog, ...localAppWorkspaceCatalog, ...providerAccountsCatalog,
  ...botsCatalog, ...chatCatalog, ...computerCatalog, ...desktopCatalog, ...extensionCatalog, ...flowCatalog,
  ...gitCatalog, ...imageCatalog, ...interactionCatalog, ...miscCatalog, ...navigationCatalog, ...runtimeCatalog,
  ...settingsCatalog, ...shellCatalog, ...uiCatalog, ...voiceSettingsCatalog, ...updateCatalog };

export function extensionLocaleCatalog(locale: typeof extensionLocales[number]): Record<string, string> {
  return Object.fromEntries(Object.entries(extensionCatalogs).flatMap(([key, entry]) =>
    entry[locale] === undefined ? [] : [[key, entry[locale]!]]));
}

export async function generateExtensionLocales(check = false): Promise<void> {
  const directory = new URL('../src/renderer/i18n/generated/', import.meta.url);
  if (!check) await mkdir(directory, { recursive: true });
  for (const locale of extensionLocales) {
    const file = new URL(`${locale}.json`, directory);
    const content = `${JSON.stringify(extensionLocaleCatalog(locale), null, 2)}\n`;
    if (check) {
      if (await readFile(file, 'utf8') !== content) throw new Error(`Stale extension translations: ${fileURLToPath(file)}. Run npm run generate:i18n.`);
    } else await writeFile(file, content);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) await generateExtensionLocales(process.argv.includes('--check'));
