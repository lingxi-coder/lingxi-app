import { voiceFlowDetail } from '../../audio/flow/controller.js';
import { tr } from '../../i18n/locale.js';
import { useI18n } from '../../i18n/useI18n.js';
import type { VoiceFlowPhase, VoiceFlowState } from '../../audio/flow/controller.js';
import { useT } from '../../theme/ThemeContext.js';
import { Icon } from '../Icon.js';
import { VoiceOrbCanvas } from './VoiceOrbCanvas.js';

function phaseLabel(phase: VoiceFlowPhase): string {
  switch (phase) {
    case 'requestingPermission':
      return tr('permission_request_title');
    case 'configurationRequired':
      return tr('voice_status_config_required');
    case 'listening':
      return tr('voice_status_listening');
    case 'recognizing':
      return tr('voice_status_recognizing');
    case 'thinking':
      return tr('voice_flow_thinking_label');
    case 'speaking':
      return tr('voice_status_speaking');
    case 'interrupting':
      return tr('desktop_flow_phase_interrupting');
    case 'paused':
      return tr('desktop_flow_phase_paused');
    case 'failed':
      return tr('desktop_flow_phase_retry');
    default:
      return tr('voice_flow_mode_label');
  }
}

function orbLabel(phase: VoiceFlowPhase): string {
  switch (phase) {
    case 'listening':
    case 'recognizing':
    case 'interrupting':
      return tr('desktop_flow_orb_finish');
    case 'thinking':
    case 'speaking':
      return tr('desktop_flow_orb_interrupt');
    case 'configurationRequired':
    case 'failed':
      return tr('desktop_flow_orb_retry');
    case 'paused':
      return tr('desktop_flow_orb_start');
    default:
      return tr('voice_flow_mode_label');
  }
}

function footerLabel(phase: VoiceFlowPhase): string {
  switch (phase) {
    case 'speaking':
      return tr('desktop_flow_footer_speaking');
    case 'thinking':
      return tr('desktop_flow_footer_thinking');
    case 'interrupting':
      return tr('desktop_flow_footer_interrupting');
    case 'configurationRequired':
      return tr('desktop_flow_footer_configuration');
    case 'failed':
      return tr('desktop_flow_footer_failed');
    default:
      return tr('desktop_flow_footer_relisten');
  }
}

export function VoiceFlowPanel({
  state,
  realtimeTurnBased = false,
  onOrb,
  onRetry,
  onOpenSettings,
  onClose,
}: {
  state: VoiceFlowState;
  realtimeTurnBased?: boolean;
  onOrb(): void;
  onRetry(): void;
  onOpenSettings(): void;
  onClose(): void;
}) {
  const t = useT();
  useI18n();
  const retryVisible = state.phase === 'configurationRequired' || state.phase === 'failed' || state.phase === 'paused';

  return (
    <section
      role="group"
      aria-label={tr('voice_flow_mode_label')}
      style={{
        position: 'relative',
        maxWidth: 980,
        height: 212,
        margin: '0 auto 12px',
        overflow: 'hidden',
        borderRadius: 26,
        border: `1px solid ${t.accentBorder}`,
        background: `linear-gradient(180deg, color-mix(in oklab, ${t.windowBg} 90%, black 10%) 0%, color-mix(in oklab, ${t.surface} 88%, ${t.windowBg} 12%) 100%)`,
        boxShadow: '0 18px 46px rgba(0,0,0,.18)',
      }}
    >
      <div
        aria-hidden="true"
        style={{
          position: 'absolute',
          inset: 0,
          background: `radial-gradient(circle at 50% 48%, color-mix(in oklab, ${t.accent} 28%, transparent) 0%, transparent 60%)`,
          opacity: state.phase === 'failed' ? 0.4 : 0.75,
        }}
      />
      <div style={{ position: 'relative', display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '15px 16px 0' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 9 }}>
          <span style={{ width: 8, height: 8, borderRadius: 999, background: state.phase === 'failed' ? t.danger : t.accent, boxShadow: `0 0 16px ${state.phase === 'failed' ? t.danger : t.accent}` }} />
          <div style={{ display: 'grid', gap: 2 }}>
            <span style={{ color: t.text, fontSize: 13, fontWeight: 650 }}>{tr('voice_flow_mode_label')}</span>
            <span style={{ color: t.text3, fontSize: 11.5 }}>{phaseLabel(state.phase)}</span>
          </div>
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          {retryVisible && (
            <button
              type="button"
              onClick={onRetry}
              style={{
                height: 30,
                padding: '0 12px',
                borderRadius: 999,
                border: `1px solid ${t.border}`,
                background: t.surfaceHover,
                color: t.text2,
                cursor: 'pointer',
                fontSize: 12,
              }}
            >
              {tr('common_retry')}
            </button>
          )}
          <button type="button" aria-label={tr('audio_guide_settings')} title={tr('audio_guide_settings')} onClick={onOpenSettings} style={{ width: 32, height: 32, display: 'grid', placeItems: 'center', borderRadius: 999, border: `1px solid ${t.border}`, background: t.surfaceHover, color: t.text3, cursor: 'pointer' }}>
            <Icon name="sliders" size={14} color="currentColor" stroke={1.9} />
          </button>
          <button type="button" aria-label={tr('voice_close_flow_mode_a11y')} title={tr('voice_close_flow_mode_a11y')} onClick={onClose} style={{ width: 32, height: 32, display: 'grid', placeItems: 'center', borderRadius: 999, border: `1px solid ${t.border}`, background: t.surfaceHover, color: t.text3, cursor: 'pointer' }}>
            <Icon name="x" size={13} color="currentColor" stroke={1.9} />
          </button>
        </div>
      </div>
      <div style={{ position: 'relative', display: 'grid', justifyItems: 'center', alignContent: 'center', gap: 10, paddingTop: 8 }}>
        <button
          type="button"
          onClick={onOrb}
          aria-label={realtimeTurnBased && (state.phase === 'thinking' || state.phase === 'speaking') ? tr('desktop_flow_turn_based_processing') : orbLabel(state.phase)}
          disabled={realtimeTurnBased && (state.phase === 'thinking' || state.phase === 'speaking')}
          title={orbLabel(state.phase)}
          style={{
            width: 138,
            height: 138,
            display: 'grid',
            placeItems: 'center',
            padding: 0,
            border: 0,
            borderRadius: 999,
            background: 'transparent',
            cursor: 'pointer',
          }}
        >
          <VoiceOrbCanvas phase={state.phase} size={124} />
        </button>
        <div
          role={state.phase === 'failed' ? 'alert' : 'status'}
          aria-live="polite"
          style={{
            maxWidth: 720,
            padding: '0 24px',
            textAlign: 'center',
            color: t.text2,
            fontSize: 12.5,
            lineHeight: 1.45,
            minHeight: 36,
          }}
        >
          {voiceFlowDetail(state)}
        </div>
      </div>
      <div style={{ position: 'absolute', left: 0, right: 0, bottom: 14, textAlign: 'center', color: t.text4, fontSize: 11 }}>
        {realtimeTurnBased ? tr('desktop_flow_turn_based_hint') : footerLabel(state.phase)}
      </div>
    </section>
  );
}
