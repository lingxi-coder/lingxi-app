import type { AudioSetupGuidance } from '../audio/audioFeatureGate';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

export function AudioSetupGuide({ guidance, onOpenSettings, onDismiss }: {
  guidance: AudioSetupGuidance;
  onOpenSettings(): void;
  onDismiss(): void;
}) {
  const t = useT();
  return <div role="status" aria-live="polite" data-testid="audio-setup-guide" style={{
    maxWidth: 'var(--conversation-width)', margin: '0 auto 10px', padding: '12px 14px',
    display: 'flex', alignItems: 'center', gap: 12, flexWrap: 'wrap',
    border: `1px solid ${t.border}`, borderRadius: 12, background: t.surface,
  }}>
    <Icon name="mic" size={18} color={t.warn} />
    <div style={{ flex: '1 1 240px', minWidth: 0 }}>
      <div style={{ color: t.text, fontSize: 13, fontWeight: 600, lineHeight: 1.4 }}>{guidance.title}</div>
      <div style={{ marginTop: 3, color: t.text3, fontSize: 12, lineHeight: 1.5 }}>{guidance.description}</div>
    </div>
    <button type="button" onClick={onOpenSettings} style={{
      border: `1px solid ${t.border}`, borderRadius: 7, padding: '6px 10px',
      background: t.surfaceHover, color: t.text, font: 'inherit', fontSize: 12, cursor: 'pointer',
    }}>语音设置</button>
    <button type="button" aria-label="关闭语音设置引导" onClick={onDismiss} style={{
      display: 'grid', placeItems: 'center', width: 28, height: 28, padding: 0,
      border: 0, borderRadius: 7, background: 'transparent', color: t.text3, cursor: 'pointer',
    }}><Icon name="x" size={13} /></button>
  </div>;
}
