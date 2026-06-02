import { useT } from '../../theme/ThemeContext';
import { Icon } from '../Icon';
import { SectionTitle } from './primitives';

export function SettingsGenericPage({ title, blurb }: { title: string; blurb: string }) {
  const t = useT();
  return (
    <div>
      <SectionTitle>{title}</SectionTitle>
      <div style={{ fontSize: 13, color: t.text3, padding: '14px 0', lineHeight: 1.6 }}>{blurb}</div>
      <div
        style={{
          marginTop: 8, padding: '40px 24px',
          background: t.surface, border: `0.5px dashed ${t.border}`, borderRadius: 12,
          display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 8,
        }}
      >
        <div style={{ width: 44, height: 44, borderRadius: 12, background: t.surfaceHover, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
          <Icon name="cog" size={22} color={t.text3} stroke={1.5} />
        </div>
        <div style={{ fontSize: 13.5, color: t.text2, fontWeight: 500 }}>{title} settings</div>
        <div style={{ fontSize: 12, color: t.text4 }}>Placeholder for the desktop mock.</div>
      </div>
    </div>
  );
}
