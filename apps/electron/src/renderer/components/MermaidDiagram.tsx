import { memo, useEffect, useRef, useState } from 'react';
import { useT } from '../theme/ThemeContext';

// Vite emits the self-contained HTML as a separate local asset. A separate
// document keeps its script policy out of the privileged renderer document.
const rendererUrl = new URL('../../../../../resources/mermaid/mermaid.html', import.meta.url).href;

export const MermaidDiagram = memo(function MermaidDiagram({ code }: { code: string }) {
  const frame = useRef<HTMLIFrameElement>(null);
  const { dark } = useT();
  const [height, setHeight] = useState(300);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    const send = () => frame.current?.contentWindow?.postMessage({ channel: 'lingxi-mermaid', source: code, dark }, '*');
    const timeout = setTimeout(() => setFailed(true), 15_000);
    const receive = (event: MessageEvent) => {
      if (event.source !== frame.current?.contentWindow || event.data?.channel !== 'lingxi-mermaid') return;
      if (event.data.type === 'ready') send();
      if (event.data.type === 'rendered' || event.data.type === 'error') clearTimeout(timeout);
      if (event.data.type === 'size' && Number.isFinite(event.data.height)) {
        setHeight(Math.min(640, Math.max(180, event.data.height)));
      }
    };
    window.addEventListener('message', receive);
    send();
    return () => { clearTimeout(timeout); window.removeEventListener('message', receive); };
  }, [code, dark]);

  if (failed) return <pre className="mono-code"><code>{code}</code></pre>;
  return <iframe ref={frame} src={rendererUrl} title="Mermaid" sandbox="allow-scripts"
    onLoad={() => frame.current?.contentWindow?.postMessage({ channel: 'lingxi-mermaid', source: code, dark }, '*')}
    onError={() => setFailed(true)}
    style={{ display: 'block', width: '100%', height, border: 0 }} />;
});
