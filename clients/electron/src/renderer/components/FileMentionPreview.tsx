import { useEffect, useRef, useState } from 'react';
import type { UseBridge } from '../bridge/useBridge';
import { useT } from '../theme/ThemeContext';
import { commandMenuStyle } from './MentionMenu';
import { Icon } from './Icon';

export function FileMentionPreview({ path, bridge, onClose }: { path: string; bridge: UseBridge; onClose(): void }) {
  const t = useT();
  const root = useRef<HTMLDivElement>(null);
  const [preview, setPreview] = useState<Awaited<ReturnType<UseBridge['previewWorkspaceFile']>> | null>(null);
  const [directory, setDirectory] = useState<string[] | null>(null);
  const [truncated, setTruncated] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    const project = bridge.activeSession?.projectPath;
    const relative = project && path.startsWith(`${project}/`) ? path.slice(project.length + 1) : path;
    const load = async () => {
      try {
        if (path.endsWith('/')) {
          const result = await bridge.searchWorkspaceFiles(relative);
          if (!cancelled) {
            setDirectory([...result.files, ...(result.directories ?? []).map((item) => `${item}/`)].filter((item) => item.startsWith(relative) && item !== relative));
            setTruncated(result.truncated);
          }
        } else {
          const result = await bridge.previewWorkspaceFile(relative);
          if (!cancelled) setPreview(result);
        }
      } catch (cause) {
        if (!cancelled) setError(cause instanceof Error ? cause.message : 'Unable to preview this reference.');
      }
    };
    void load();
    return () => { cancelled = true; };
  }, [path, bridge.activeSession?.projectPath, bridge.previewWorkspaceFile, bridge.searchWorkspaceFiles]);
  useEffect(() => {
    const keyDown = (event: KeyboardEvent) => { if (event.key === 'Escape') { event.preventDefault(); onClose(); } };
    const pointerDown = (event: PointerEvent) => { if (event.target instanceof Node && !root.current?.contains(event.target)) onClose(); };
    document.addEventListener('keydown', keyDown);
    document.addEventListener('pointerdown', pointerDown);
    return () => { document.removeEventListener('keydown', keyDown); document.removeEventListener('pointerdown', pointerDown); };
  }, [onClose]);
  return <div ref={root} className="slash-command-menu" role="dialog" aria-label="Reference preview" style={commandMenuStyle(t)}>
    <div className="slash-command-header"><Icon name={path.endsWith('/') ? 'folder' : 'file'} size={17} /><strong className="slash-command-query" title={path}>{path}</strong><button className="mention-menu-control" type="button" aria-label="Close reference preview" onClick={onClose} style={{ marginLeft: 'auto' }}><Icon name="x" size={14} /></button></div>
    <div className="slash-command-list" style={{ padding: 16 }}>
      {error ? <div role="alert">{error}</div> : directory ? <div>{directory.length ? directory.map((item) => <div key={item} className="mono" style={{ fontSize: 12, padding: '4px 0', overflowWrap: 'anywhere' }}>{item}</div>) : 'This folder is empty.'}{truncated && <p>More entries are available. Narrow the file search to see them.</p>}</div>
        : !preview ? <div role="status">Loading preview…</div>
          : preview.kind === 'binary' ? <div>Binary file · {preview.size.toLocaleString()} bytes</div>
            : <><pre style={{ margin: 0, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', fontSize: 12 }}>{preview.content}</pre>{preview.truncated && <p>Preview truncated.</p>}</>}
    </div>
  </div>;
}
