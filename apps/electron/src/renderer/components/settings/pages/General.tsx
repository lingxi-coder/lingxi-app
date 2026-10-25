import { tr } from '../../../i18n/locale';
import { Card, Row } from '../rows';
import { useI18n } from '../../../i18n/useI18n';
import type { LanguagePreference } from '../../../i18n/locale';
import { useT } from '../../../theme/ThemeContext';
import { Icon } from '../../Icon';
import type { PageContentProps } from '../SettingsScreen';

const THEME_PREFERENCE_LABELS: Record<'system' | 'light' | 'dark', string> = {
  system: 'settings_appearance_system', light: 'settings_appearance_light', dark: 'settings_appearance_dark',
};

function EntryRow({ icon, title, desc, onClick }: { icon: string; title: string; desc: string; onClick(): void }) {
  const t = useT();
  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        width: '100%', display: 'flex', alignItems: 'center', gap: 12, padding: '14px 18px',
        border: 'none', borderTop: `0.5px solid ${t.border}`, background: 'transparent', cursor: 'pointer',
        textAlign: 'left', fontFamily: 'inherit', color: t.text,
      }}
    >
      <div style={{
        width: 30, height: 30, borderRadius: 8, background: t.surfaceHover, flexShrink: 0,
        display: 'flex', alignItems: 'center', justifyContent: 'center',
      }}>
        <Icon name={icon} size={15} color={t.text3} stroke={1.7} />
      </div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontSize: 13.5, fontWeight: 500 }}>{title}</div>
        <div style={{ fontSize: 12, color: t.text3, marginTop: 2 }}>{desc}</div>
      </div>
      <Icon name="chevronR" size={13} color={t.text4} stroke={2} />
    </button>
  );
}

/** Device-local language preference and shortcuts to common settings. */
export function General({ bridge, onNavigate }: PageContentProps) {
  const { preference: language, setPreference } = useI18n();
  const t = useT();
  const preference = (bridge.bootstrap?.settings?.theme ?? 'system') as 'system' | 'light' | 'dark';
  const projectCount = bridge.bootstrap?.settings?.projects?.length ?? 0;

  return (
    <>
    <Card title={tr('settings_language_title')}>
      <Row title={tr('desktop_interface_language')} desc={tr('desktop_language_hint')} align="center">
        <select aria-label={tr('settings_language_title')} value={language}
          onChange={event => setPreference(event.target.value as LanguagePreference)}
          style={{ maxWidth: '100%', minWidth: 130, padding: '7px 10px', borderRadius: 7, border: `1px solid ${t.border}`, background: t.surface, color: t.text, font: 'inherit' }}>
          <option value="">{tr('settings_appearance_system')}</option>
          <option value="zh-Hans">简体中文</option><option value="zh-Hant">繁體中文</option>
          <option value="en">English</option><option value="ja">日本語</option><option value="ko">한국어</option>
        </select>
      </Row>
    </Card>
    <Card title={tr('desktop_quick_links')}>
      <EntryRow icon="sun" title={tr('settings_appearance')} desc={tr('desktop_current_theme', tr(THEME_PREFERENCE_LABELS[preference]))} onClick={() => onNavigate('appearance')} />
      <EntryRow icon="folder" title={tr('desktop_project_trust')} desc={tr('desktop_added_projects', projectCount)} onClick={() => onNavigate('projects')} />
      <EntryRow icon="activity" title={tr('desktop_diagnostics')} desc={tr('desktop_diagnostics_sub')} onClick={() => onNavigate('diagnostics')} />
      <EntryRow icon="info" title={tr('settings_group_about')} desc={tr('desktop_version_info')} onClick={() => onNavigate('about')} />
    </Card>
    </>
  );
}
