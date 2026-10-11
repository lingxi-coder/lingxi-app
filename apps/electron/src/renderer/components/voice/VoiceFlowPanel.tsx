import type { VoiceFlowPhase, VoiceFlowState } from '../../audio/flow/controller.js';
import { useT } from '../../theme/ThemeContext.js';
import { Icon } from '../Icon.js';
import { VoiceOrbCanvas } from './VoiceOrbCanvas.js';

function phaseLabel(phase: VoiceFlowPhase): string {
  switch (phase) {
    case 'requestingPermission':
      return '请求权限';
    case 'configurationRequired':
      return '需要配置';
    case 'listening':
      return '正在聆听';
    case 'recognizing':
      return '识别中';
    case 'thinking':
      return '正在思考';
    case 'speaking':
      return '正在播报';
    case 'interrupting':
      return '准备插话';
    case 'paused':
      return '待命';
    case 'failed':
      return '需要重试';
    default:
      return '心流模式';
  }
}

function orbLabel(phase: VoiceFlowPhase): string {
  switch (phase) {
    case 'listening':
    case 'recognizing':
    case 'interrupting':
      return '结束当前聆听';
    case 'thinking':
    case 'speaking':
      return '打断当前回复并说出新问题';
    case 'configurationRequired':
    case 'failed':
      return '重试心流模式';
    case 'paused':
      return '开始心流模式';
    default:
      return '心流模式';
  }
}

function footerLabel(phase: VoiceFlowPhase): string {
  switch (phase) {
    case 'speaking':
      return '点击 Orb 可以打断并改问新的问题';
    case 'thinking':
      return '回复生成中，点击 Orb 可以直接插话';
    case 'interrupting':
      return '说完后会只取消当前心流回合并发送替代问题';
    case 'configurationRequired':
      return '先修复权限或模型问题，再继续完整闭环';
    case 'failed':
      return '当前回合已停止，可重试或打开语音设置';
    default:
      return '心流会在播报结束后自动重新聆听';
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
  const retryVisible = state.phase === 'configurationRequired' || state.phase === 'failed' || state.phase === 'paused';

  return (
    <section
      role="group"
      aria-label="心流模式"
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
            <span style={{ color: t.text, fontSize: 13, fontWeight: 650 }}>心流模式</span>
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
              重试
            </button>
          )}
          <button type="button" aria-label="打开语音设置" title="语音设置" onClick={onOpenSettings} style={{ width: 32, height: 32, display: 'grid', placeItems: 'center', borderRadius: 999, border: `1px solid ${t.border}`, background: t.surfaceHover, color: t.text3, cursor: 'pointer' }}>
            <Icon name="sliders" size={14} color="currentColor" stroke={1.9} />
          </button>
          <button type="button" aria-label="关闭心流模式" title="关闭心流模式" onClick={onClose} style={{ width: 32, height: 32, display: 'grid', placeItems: 'center', borderRadius: 999, border: `1px solid ${t.border}`, background: t.surfaceHover, color: t.text3, cursor: 'pointer' }}>
            <Icon name="x" size={13} color="currentColor" stroke={1.9} />
          </button>
        </div>
      </div>
      <div style={{ position: 'relative', display: 'grid', justifyItems: 'center', alignContent: 'center', gap: 10, paddingTop: 8 }}>
        <button
          type="button"
          onClick={onOrb}
          aria-label={realtimeTurnBased && (state.phase === 'thinking' || state.phase === 'speaking') ? '轮流说话模式正在处理回复' : orbLabel(state.phase)}
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
          {state.detail}
        </div>
      </div>
      <div style={{ position: 'absolute', left: 0, right: 0, bottom: 14, textAlign: 'center', color: t.text4, fontSize: 11 }}>
        {realtimeTurnBased ? '轮流说话 · 轻点 Orb 结束聆听，播报后自动重新聆听' : footerLabel(state.phase)}
      </div>
    </section>
  );
}
