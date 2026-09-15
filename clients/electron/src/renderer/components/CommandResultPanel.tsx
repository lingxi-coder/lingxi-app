import { useEffect, useId, useRef, type CSSProperties } from 'react';
import { commandPresentation, type CommandRunItem } from '../model/runItem';
import { useT } from '../theme/ThemeContext';
import { CommandBody } from './CommandOutput';
import { Icon } from './Icon';

/** Ephemeral utility output, intentionally independent of the conversation transcript. */
export function CommandResultPanel({ item, onClose }: { item: CommandRunItem; onClose(): void }) {
  const t = useT();
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const presentation = commandPresentation(item);
  useEffect(() => {
    const dialog = ref.current;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog?.showModal();
    return () => {
      dialog?.close();
      if (previous?.isConnected) previous.focus();
    };
  }, []);
  return (
    <dialog ref={ref} className="command-utility-dialog" aria-labelledby={titleId}
      style={{ '--command-surface': t.surface, '--command-surface-muted': t.surfaceHover,
        '--command-text': t.text, '--command-text-muted': t.text3, '--command-ring': t.border,
        '--command-accent': t.accent, '--command-danger': t.danger,
      } as CSSProperties}
      onCancel={(event) => { event.preventDefault(); onClose(); }}
      onClick={(event) => { if (event.target === event.currentTarget) {
        const rect = event.currentTarget.getBoundingClientRect();
        if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) onClose();
      } }}>
      <header className="command-utility-header">
        <div><h2 id={titleId}>{presentation.title}</h2><p>{item.name || 'Command output'}</p></div>
        <button type="button" autoFocus aria-label="Close command result" onClick={onClose}><Icon name="x" size={18} /></button>
      </header>
      <div className="command-utility-content" role={item.isError ? 'alert' : undefined} data-error={item.isError || undefined}>
        <CommandBody item={item} />
      </div>
    </dialog>
  );
}
