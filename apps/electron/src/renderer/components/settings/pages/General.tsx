import { Card } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { Icon } from '../../Icon';
import type { PageContentProps } from '../SettingsScreen';

const THEME_PREFERENCE_LABELS: Record<'system' | 'light' | 'dark', string> = {
  system: '跟随系统', light: '浅色', dark: '深色',
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

/**
 * This round only carries cross-page entry points, not new unimplemented
 * toggles: the old mock General settings page (retired in Task 20) had many (run-on-startup,
 * shortcuts, browser-use switches, …) with no real IPC behind any of them —
 * building "working" controls on top of that here would just add more
 * computed-but-never-wired surface, the exact defect class this shell
 * exists to remove. Each row below reads real bridge state and really
 * navigates (via `onNavigate`, which the shell wires to its own page state)
 * rather than being a link to nowhere.
 */
export function General({ bridge, onNavigate }: PageContentProps) {
  const preference = (bridge.bootstrap?.settings?.theme ?? 'system') as 'system' | 'light' | 'dark';
  const projectCount = bridge.bootstrap?.settings?.projects?.length ?? 0;

  return (
    <Card title="快速入口">
      <EntryRow icon="sun" title="外观" desc={`当前：${THEME_PREFERENCE_LABELS[preference]}`} onClick={() => onNavigate('appearance')} />
      <EntryRow icon="folder" title="项目与信任" desc={`已添加 ${projectCount} 个项目`} onClick={() => onNavigate('projects')} />
      <EntryRow icon="activity" title="诊断" desc="日志、导出与手动重启" onClick={() => onNavigate('diagnostics')} />
      <EntryRow icon="info" title="关于" desc="版本信息" onClick={() => onNavigate('about')} />
    </Card>
  );
}
