const diagram = document.getElementById('diagram');
const viewport = document.getElementById('viewport');
const code = document.getElementById('code');
const error = document.getElementById('error');
const sourceButton = document.getElementById('source');
const chinese = navigator.language.toLowerCase().startsWith('zh');
const labels = chinese
  ? ['缩小', '放大', '重置缩放', '源码', '流程图暂时无法渲染，可查看或复制源码。']
  : ['Zoom out', 'Zoom in', 'Reset zoom', 'Source code', 'Unable to render this diagram. The source is available below.'];
['out', 'in', 'fit', 'source'].forEach((id, index) => {
  const button = document.getElementById(id);
  button.title = labels[index];
  button.setAttribute('aria-label', labels[index]);
});
let current;
let revision = 0;
let pending;
let rendering = false;
let zoom = 1;
let showSource = false;
let svg;
let naturalWidth = 0;
let naturalHeight = 0;

function notify(message) {
  const data = { channel: 'lingxi-mermaid', ...message };
  if (window.parent !== window) window.parent.postMessage(data, '*');
  window.webkit?.messageHandlers?.mermaid?.postMessage(data);
  // Android only exposes a one-way, origin-scoped WebMessageListener.
  window.mermaidHost?.postMessage(JSON.stringify(data));
}

function resize() {
  if (!svg) return;
  const width = Math.max(120, Math.min(naturalWidth, viewport.clientWidth - 24));
  svg.style.width = `${width * zoom}px`;
  svg.style.height = `${width / naturalWidth * naturalHeight * zoom}px`;
  notify({ type: 'size', revision, height: Math.min(640, Math.max(180, width / naturalWidth * naturalHeight + 74)) });
}

function toggleSource(value) {
  showSource = value;
  diagram.hidden = value;
  code.hidden = !value;
  sourceButton.setAttribute('aria-pressed', String(value));
}
sourceButton.onclick = () => toggleSource(!showSource);
document.getElementById('in').onclick = () => { zoom = Math.min(4, zoom * 1.25); resize(); };
document.getElementById('out').onclick = () => { zoom = Math.max(0.25, zoom / 1.25); resize(); };
document.getElementById('fit').onclick = () => { zoom = 1; resize(); viewport.scrollTo(0, 0); };
new ResizeObserver(resize).observe(viewport);

async function drain() {
  if (rendering) return;
  rendering = true;
  while (pending) {
    const input = pending;
    pending = undefined;
    const ownRevision = input.revision;
    const staging = document.createElement('div');
    staging.style.cssText = 'position:absolute;left:0;top:0;visibility:hidden;pointer-events:none';
    document.body.append(staging);
    try {
      if (input.source.length > 50_000) throw new Error('Diagram too large');
      mermaid.initialize({
        startOnLoad: false, securityLevel: 'strict', suppressErrorRendering: true,
        maxTextSize: 50_000, maxEdges: 500, htmlLabels: false,
        theme: input.dark ? 'dark' : 'neutral', fontFamily: 'system-ui, sans-serif',
        // Author directives cannot replace host policy or inject CSS/HTML.
        secure: ['secure', 'securityLevel', 'startOnLoad', 'maxTextSize', 'maxEdges',
          'suppressErrorRendering', 'htmlLabels', 'theme', 'themeCSS', 'themeVariables',
          'fontFamily', 'dompurifyConfig', 'flowchart'],
        flowchart: { htmlLabels: false, useMaxWidth: false },
      });
      const result = await mermaid.render(`mermaid-${ownRevision}`, input.source, staging);
      if (ownRevision !== revision) continue;
      diagram.innerHTML = result.svg;
      // No authored links, images or event bindings in transcript diagrams.
      diagram.querySelectorAll('a').forEach((node) => node.replaceWith(...node.childNodes));
      diagram.querySelectorAll('image, script, foreignObject').forEach((node) => node.remove());
      svg = diagram.querySelector('svg');
      const box = svg?.viewBox.baseVal;
      if (!box || box.width <= 0 || box.height <= 0) throw new Error('Invalid SVG');
      naturalWidth = box.width;
      naturalHeight = box.height;
      error.hidden = true;
      zoom = 1;
      toggleSource(false);
      resize();
      notify({ type: 'rendered', revision });
    } catch {
      if (ownRevision !== revision) continue;
      svg = undefined;
      diagram.replaceChildren();
      error.textContent = labels[4];
      error.hidden = false;
      toggleSource(true);
      notify({ type: 'size', revision, height: 300 });
      notify({ type: 'error', revision });
    } finally {
      staging.remove();
    }
  }
  rendering = false;
}

window.renderMermaid = (input) => {
  if (!input || typeof input.source !== 'string' || typeof input.dark !== 'boolean') return;
  if (current?.source === input.source && current.dark === input.dark) return;
  current = { source: input.source, dark: input.dark };
  document.documentElement.classList.toggle('dark', input.dark);
  code.textContent = input.source;
  pending = { ...current, revision: ++revision };
  void drain();
};
window.addEventListener('message', (event) => {
  if (event.source === window.parent && event.data?.channel === 'lingxi-mermaid') {
    window.renderMermaid(event.data);
  }
});
// Diagram-authored navigation is never actionable.
document.addEventListener('click', (event) => {
  if (event.target.closest?.('a')) event.preventDefault();
});
notify({ type: 'ready' });
