import { Card, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { ThemeMode } from '../../../theme/tokens';
import type { PageContentProps } from '../SettingsScreen';

export interface AppearanceOption {
  id: 'system' | 'light' | 'dark';
  label: string;
}

/**
 * Exactly the three choices the picker offers, in the order they render.
 * Pure and exported so the order and membership can be pinned without
 * mounting anything — this is the one decision this page makes.
 */
export function appearanceOptions(): AppearanceOption[] {
  return [
    { id: 'system', label: '跟随系统' },
    { id: 'light', label: '浅色' },
    { id: 'dark', label: '深色' },
  ];
}

export function Appearance({ bridge, onTheme }: PageContentProps) {
  const t = useT();
  // `settings.theme` is the PREFERENCE (widened to include `'system'` by
  // this same task); it is not the same value as `PageContentProps.theme`,
  // which is the already-RESOLVED two-valued palette used to paint the rest
  // of the app chrome. A project that has never set a preference reads as
  // `undefined`, which this page treats as "system" for display — the
  // closest honest reading of "no explicit choice has been made".
  const preference = bridge.bootstrap?.settings?.theme ?? 'system';

  const select = (id: AppearanceOption['id']) => {
    if (id === 'system') {
      // `onTheme` only accepts the two-valued `ThemeMode` (see `tokens.ts`'s
      // `ThemeMode`/`ThemePreference` split) — it cannot carry `'system'`.
      // Persisting the preference directly is enough: `App.tsx` already
      // watches `bridge.bootstrap.settings.theme` and resolves + applies it
      // (including live `matchMedia` tracking) the moment this patch lands.
      void bridge.setThemePreference('system').catch(() => undefined);
      return;
    }
    // `onTheme` is `changeTheme` in `App.tsx`: it updates the resolved
    // palette AND persists the preference (`bridge.setThemePreference`) in
    // one call. Also calling `bridge.setThemePreference` here would just
    // duplicate that same write.
    onTheme(id as ThemeMode);
  };

  return (
    <Card title="外观">
      <Row title="主题" desc="选择浅色或深色外观，或跟随系统设置自动切换。" align="center">
        {/* Not `primitives.tsx`'s `Segmented`: that file is legacy scaffolding
            used only by the eight mock settings pages a later task deletes —
            coupling this page to it would work against the plan it's built
            to support. This inline three-way pill mirrors the same look
            `SettingsScreen.tsx`'s own `LayerSwitcher` already uses. */}
        <div style={{ display: 'inline-flex', padding: 3, gap: 2, borderRadius: 9, background: t.sidebarBg, border: `0.5px solid ${t.border}` }}>
          {appearanceOptions().map((option) => {
            const active = preference === option.id;
            return (
              <button
                key={option.id}
                type="button"
                data-appearance-option={option.id}
                aria-pressed={active}
                onClick={() => select(option.id)}
                style={{
                  padding: '5px 14px', borderRadius: 7, border: 'none', fontFamily: 'inherit',
                  cursor: 'pointer',
                  background: active ? t.surface : 'transparent',
                  color: active ? t.text : t.text3,
                  fontSize: 12.5, fontWeight: active ? 600 : 500,
                }}
              >
                {option.label}
              </button>
            );
          })}
        </div>
      </Row>
    </Card>
  );
}
