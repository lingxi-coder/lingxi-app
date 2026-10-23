import { useEffect, useRef, useState } from 'react';
import type { ProjectFolderSelection } from '../../shared/projectCreation';
import { DesktopDialog } from './DesktopDialog';
import { Icon } from './Icon';
import './CreateProjectDialog.css';

export function CreateProjectDialog({ onPickFolder, onCreate, onClose }: {
  onPickFolder(): Promise<ProjectFolderSelection | null>;
  onCreate(selectionId: string): Promise<void>;
  onClose(): void;
}) {
  const dialog = useRef<HTMLDivElement>(null);
  const nameInput = useRef<HTMLInputElement>(null);
  const requestVersion = useRef(0);
  const [selection, setSelection] = useState<ProjectFolderSelection | null>(null);
  const [busy, setBusy] = useState<'selecting' | 'creating' | null>(null);
  const [error, setError] = useState('');
  const name = selection?.path.split(/[\\/]/).filter(Boolean).at(-1) ?? selection?.path ?? '';

  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog.current?.setAttribute('aria-modal', 'true');
    nameInput.current?.focus();
    return () => {
      requestVersion.current++;
      if (opener?.isConnected && !opener.closest('[inert]')) opener.focus();
    };
  }, []);
  useEffect(() => {
    if (busy) dialog.current?.focus();
  }, [busy]);

  const pickFolder = async () => {
    if (busy) return;
    const version = ++requestVersion.current;
    setBusy('selecting'); setError('');
    try {
      const next = await onPickFolder();
      if (requestVersion.current === version && next) setSelection(next);
    } catch (cause) {
      if (requestVersion.current === version) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (requestVersion.current === version) setBusy(null);
    }
  };
  const create = async () => {
    if (!selection || busy) return;
    const version = ++requestVersion.current;
    setBusy('creating'); setError('');
    try { await onCreate(selection.id); }
    catch (cause) {
      if (requestVersion.current === version) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (requestVersion.current === version) setBusy(null);
    }
  };

  return <DesktopDialog ref={dialog} tabIndex={-1} title="Create project" ariaLabel="Create project"
    className="create-project-dialog" onEscape={() => { if (!busy) onClose(); }}
    onKeyDown={(event) => {
      event.stopPropagation();
      if (event.key !== 'Tab') return;
      const controls = dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled)');
      const first = controls?.[0], last = controls?.[controls.length - 1];
      if (!first || !last) { event.preventDefault(); dialog.current?.focus(); return; }
      if (document.activeElement === dialog.current || !dialog.current?.contains(document.activeElement)) {
        event.preventDefault(); (event.shiftKey ? last : first).focus();
      } else if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    }}>
    <button className="create-project-close" type="button" aria-label="Close create project" disabled={Boolean(busy)} onClick={onClose}><Icon name="x" size={17} stroke={1.6} /></button>
    <form onSubmit={(event) => { event.preventDefault(); void create(); }} aria-busy={Boolean(busy)}>
      <label className="create-project-name">
        <span className="create-project-name-icon" aria-hidden="true"><Icon name="folder" size={17} stroke={1.7} /></span>
        <input ref={nameInput} aria-label="Project name" title="Uses the source folder’s name" placeholder="Project name"
          value={name} readOnly disabled={Boolean(busy)} />
      </label>
      <div className="create-project-sources-heading" id="create-project-sources-heading">Source folders</div>
      <div className="create-project-sources" role="group" aria-labelledby="create-project-sources-heading">
        {selection ? <div className="create-project-selected-folder">
          <Icon name="folder" size={22} stroke={1.6} />
          <span className="create-project-folder-copy"><strong>{name}</strong><span title={selection.path}>{selection.path}</span></span>
          <button type="button" className="create-project-remove" aria-label="Remove source folder" disabled={Boolean(busy)}
            onClick={() => { setSelection(null); setError(''); }}><Icon name="x" size={15} /></button>
        </div> : <div className="create-project-source-location"><span>Add a folder</span>
          <span className="create-project-location-select"><select aria-label="Source folder location" defaultValue="local" disabled={Boolean(busy)}><option value="local">on this computer</option></select><Icon name="chevron" size={14} stroke={1.7} /></span>
        </div>}
        <button className="create-project-add-folder" type="button" disabled={Boolean(busy)} onClick={() => void pickFolder()}>
          <Icon name="folderPlus" size={17} stroke={1.7} />{busy === 'selecting' ? 'Choosing…' : selection ? 'Change' : 'Add'}
        </button>
      </div>
      {error && <p role="alert" className="create-project-error">{error}</p>}
      <div className="create-project-actions">
        <button type="button" className="create-project-cancel" disabled={Boolean(busy)} onClick={onClose}>Cancel</button>
        <button type="submit" className="create-project-submit" disabled={!selection || Boolean(busy)}>{busy === 'creating' ? 'Creating…' : 'Create project'}</button>
      </div>
    </form>
  </DesktopDialog>;
}
