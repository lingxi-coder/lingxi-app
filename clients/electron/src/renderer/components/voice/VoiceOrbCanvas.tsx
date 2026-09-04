import { useEffect, useRef, useState } from 'react';

import type { VoiceFlowPhase } from '../../audio/flow/controller.js';
import { useT } from '../../theme/ThemeContext.js';

function phaseColor(phase: VoiceFlowPhase, accent: string, danger: string, text: string): string {
  switch (phase) {
    case 'failed':
      return danger;
    case 'configurationRequired':
    case 'interrupting':
      return '#f59e0b';
    case 'paused':
      return text;
    default:
      return accent;
  }
}

export function canvasColorWithAlpha(color: string, alpha: number): string {
  const normalized = color.trim();
  const boundedAlpha = Math.min(1, Math.max(0, alpha));
  const hex = normalized.match(/^#([\da-f]{6})$/i);
  if (hex) {
    const alphaHex = Math.round(boundedAlpha * 255).toString(16).padStart(2, '0');
    return `#${hex[1]}${alphaHex}`;
  }
  const modernFunction = normalized.match(/^(oklch|oklab|lch|lab)\((.*)\)$/i);
  if (modernFunction) {
    const body = modernFunction[2].replace(/\s*\/\s*[^)]+$/, '').trim();
    return `${modernFunction[1]}(${body} / ${boundedAlpha})`;
  }
  return normalized;
}

export function VoiceOrbCanvas({ phase, size = 124 }: { phase: VoiceFlowPhase; size?: number }) {
  const t = useT();
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [reducedMotion, setReducedMotion] = useState(false);

  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia('(prefers-reduced-motion: reduce)');
    const update = () => setReducedMotion(media.matches);
    update();
    media.addEventListener?.('change', update);
    return () => media.removeEventListener?.('change', update);
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const context = canvas.getContext('2d');
    if (!context) return;

    const devicePixelRatio = Math.max(1, typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1);
    const cssSize = size;
    canvas.width = Math.round(cssSize * devicePixelRatio);
    canvas.height = Math.round(cssSize * devicePixelRatio);
    context.setTransform(devicePixelRatio, 0, 0, devicePixelRatio, 0, 0);

    const color = phaseColor(phase, t.accent, t.danger, t.text3);
    const background = t.windowBg;
    const center = cssSize / 2;
    const coreRadius = cssSize * 0.24;
    let frame = 0;

    const draw = (timestamp: number) => {
      const time = reducedMotion ? 0.45 : timestamp / 1000;
      const pulse = reducedMotion ? 0.65 : (Math.sin(time * 2.4) + 1) / 2;
      context.clearRect(0, 0, cssSize, cssSize);

      const glow = context.createRadialGradient(center, center, coreRadius * 0.4, center, center, cssSize * 0.48);
      glow.addColorStop(0, canvasColorWithAlpha(color, 0.27));
      glow.addColorStop(0.58, canvasColorWithAlpha(color, 0.09));
      glow.addColorStop(1, 'transparent');
      context.fillStyle = glow;
      context.beginPath();
      context.arc(center, center, cssSize * 0.48, 0, Math.PI * 2);
      context.fill();

      const ringCount = phase === 'speaking' ? 4 : 3;
      for (let index = 0; index < ringCount; index += 1) {
        const ringPulse = (pulse + index / ringCount) % 1;
        const radius = coreRadius + 10 + index * 10 + ringPulse * (phase === 'paused' ? 2 : 7);
        context.strokeStyle = canvasColorWithAlpha(color, phase === 'paused' ? 0.13 : 0.2);
        context.lineWidth = 1.5;
        context.globalAlpha = phase === 'paused' ? 0.35 : Math.max(0.12, 0.45 - index * 0.1 - ringPulse * 0.15);
        context.beginPath();
        context.arc(center, center, radius, 0, Math.PI * 2);
        context.stroke();
      }
      context.globalAlpha = 1;

      context.fillStyle = background;
      context.beginPath();
      context.arc(center, center, coreRadius + 8, 0, Math.PI * 2);
      context.fill();

      const innerGlow = context.createRadialGradient(center, center, 6, center, center, coreRadius + 2);
      innerGlow.addColorStop(0, canvasColorWithAlpha(color, 0.93));
      innerGlow.addColorStop(1, canvasColorWithAlpha(color, 0.53));
      context.fillStyle = innerGlow;
      context.beginPath();
      context.arc(center, center, coreRadius, 0, Math.PI * 2);
      context.fill();

      context.fillStyle = '#ffffff';
      context.globalAlpha = phase === 'failed' ? 0.9 : 0.82;
      context.beginPath();
      context.arc(center, center, coreRadius * 0.45 + pulse * 3, 0, Math.PI * 2);
      context.fill();
      context.globalAlpha = 1;

      if (phase === 'thinking' || phase === 'requestingPermission') {
        context.strokeStyle = '#ffffff';
        context.lineWidth = 2;
        context.globalAlpha = 0.75;
        context.beginPath();
        context.arc(center, center, coreRadius + 14, time * 1.5, time * 1.5 + Math.PI * 1.2);
        context.stroke();
        context.globalAlpha = 1;
      } else if (phase === 'speaking' || phase === 'recognizing' || phase === 'interrupting') {
        const barHeights = [12, 20, 30, 20, 12].map((base, index) => (
          reducedMotion ? base : base + Math.sin(time * 5 + index * 0.7) * 5
        ));
        context.fillStyle = '#ffffff';
        barHeights.forEach((height, index) => {
          const width = 4;
          const gap = 5.5;
          const x = center - ((barHeights.length - 1) * (width + gap)) / 2 + index * (width + gap);
          const y = center - height / 2;
          context.globalAlpha = 0.8 - index * 0.08;
          context.beginPath();
          context.roundRect(x, y, width, height, 2);
          context.fill();
        });
        context.globalAlpha = 1;
      } else if (phase === 'failed' || phase === 'configurationRequired') {
        context.strokeStyle = '#ffffff';
        context.lineWidth = 2.4;
        context.beginPath();
        context.moveTo(center - 10, center - 10);
        context.lineTo(center + 10, center + 10);
        context.moveTo(center + 10, center - 10);
        context.lineTo(center - 10, center + 10);
        context.stroke();
      }

      frame = reducedMotion ? 0 : window.requestAnimationFrame(draw);
    };

    draw(0);
    return () => {
      if (frame) window.cancelAnimationFrame(frame);
    };
  }, [phase, reducedMotion, size, t.accent, t.danger, t.text3, t.windowBg]);

  return <canvas ref={canvasRef} width={size} height={size} aria-hidden="true" style={{ width: size, height: size, display: 'block' }} />;
}
