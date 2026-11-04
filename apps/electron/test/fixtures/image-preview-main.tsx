import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { ImagePreview } from '../../src/renderer/components/ImagePreview';
import { Stage } from '../../src/renderer/components/Stage';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

localStorage.setItem('lingxi.desktop.language', 'en');
function sampleImage(width: number, height: number) {
  const canvas = document.createElement('canvas');
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext('2d')!;
  ctx.fillStyle = '#f2f2f7'; ctx.fillRect(0, 0, width, height);
  ctx.fillStyle = '#111'; ctx.font = '500 28px system-ui'; ctx.fillText('9:41', 36, 50);
  ctx.fillStyle = '#fff'; ctx.beginPath(); ctx.roundRect(24, 130, width - 48, 100, 32); ctx.fill();
  ctx.fillStyle = '#111'; ctx.font = '36px system-ui'; ctx.fillText('模型设置', 60, 194);
  ctx.fillStyle = '#888'; ctx.fillText('Sonnet  ›', width - 240, 194);
  ctx.font = '600 32px system-ui'; ctx.fillText('Anthropic', 60, 312);
  ctx.fillStyle = '#fff'; ctx.beginPath(); ctx.roundRect(24, 350, width - 48, 830, 32); ctx.fill();
  ['Claude Sonnet 5', 'Claude Opus 4.8', 'Claude Haiku 4.5', 'Claude Fable 5.1'].forEach((label, i) => {
    const top = 420 + i * 205;
    ctx.fillStyle = '#111'; ctx.font = '36px system-ui'; ctx.fillText(label, 60, top);
    ctx.fillStyle = '#8170ed'; ctx.font = '28px system-ui'; ctx.fillText('状态待验证', 60, top + 48);
    ctx.fillStyle = '#8e8e93'; ctx.fillText(label.toLowerCase().replaceAll(' ', '-'), 60, top + 96);
    if (i === 0) { ctx.fillStyle = '#6b52f5'; ctx.fillText('✓', width - 94, top + 48); }
    if (i < 3) { ctx.strokeStyle = '#e5e5ea'; ctx.beginPath(); ctx.moveTo(60, top + 145); ctx.lineTo(width - 60, top + 145); ctx.stroke(); }
  });
  ctx.fillStyle = '#888'; ctx.font = '32px system-ui'; ctx.fillText('OpenAI · API key', 60, 1280);
  ctx.fillStyle = '#fff'; ctx.beginPath(); ctx.roundRect(24, 1320, width - 48, 280, 32); ctx.fill();
  return canvas.toDataURL('image/png');
}
const portrait = sampleImage(780, 1688);
const landscape = sampleImage(1600, 900);
const palette = tokens(false);
createRoot(document.getElementById('root')!).render(<StrictMode><Theme.Provider value={palette}>
  <main style={{ padding: 32, background: palette.stageBg, color: palette.text, height: '100vh' }}>
    <h1 style={{ marginBottom: 24 }}>LingXi · Image preview</h1>
    <section id="portrait" style={{ height: 450 }}><Stage liveItems={[{ type: 'narration', id: 'image-message', role: 'user', text: 'Preview this screenshot', images: [{ url: portrait, media_type: 'image/png' }] }]} sessionKey="image-preview-fixture" /></section>
    <div style={{ display: 'flex', gap: 16 }}>
      <div id="landscape" style={{ width: 160 }}><ImagePreview src={landscape} alt="landscape.png" /></div>
      <div id="broken"><ImagePreview src="data:image/png;base64,invalid" alt="broken.png" /></div>
    </div>
    <button id="background" onClick={() => document.body.dataset.clicked = 'true'}>Background action</button>
  </main>
</Theme.Provider></StrictMode>);
