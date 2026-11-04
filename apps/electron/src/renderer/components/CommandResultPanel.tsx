import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { useEffect, useLayoutEffect, useId, useRef, useState, type CSSProperties } from 'react';
import { commandName, commandPresentation, type CommandRunItem } from '../model/runItem';
import { useT } from '../theme/ThemeContext';
import { CommandBody } from './CommandOutput';
import { Icon } from './Icon';
import { CommandIcon } from './CommandIdentity';
import { commandPaletteColor } from './commandPaletteIcons';

const useBrowserLayoutEffect = typeof window === 'undefined' ? useEffect : useLayoutEffect;

/** Ephemeral utility output, intentionally independent of the conversation transcript. */
export function CommandResultPanel({ item, onClose, onDismiss = onClose }: {
  item: CommandRunItem; onClose(): void; onDismiss?(): void;
}) {
  const t = useT();
  useI18n();
  const ref = useRef<HTMLDivElement>(null);
  const [maxHeight, setMaxHeight] = useState(448);
  const titleId = useId();
  const presentation = commandPresentation(item);
  const commandColor = commandPaletteColor(item.name, t.dark);
  useBrowserLayoutEffect(() => {
    const anchor = ref.current?.parentElement;
    if (!anchor) return;
    const measure = () => setMaxHeight(Math.max(0, anchor.getBoundingClientRect().top - 64));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(anchor);
    window.addEventListener('resize', measure);
    return () => { observer.disconnect(); window.removeEventListener('resize', measure); };
  }, []);
  useEffect(() => {
    const keyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented || event.isComposing) return;
      event.preventDefault();
      event.stopPropagation();
      onClose();
    };
    const pointerDown = (event: PointerEvent) => {
      if (event.target instanceof Node && !ref.current?.contains(event.target)) onDismiss();
    };
    document.addEventListener('keydown', keyDown);
    document.addEventListener('pointerdown', pointerDown);
    return () => {
      document.removeEventListener('keydown', keyDown);
      document.removeEventListener('pointerdown', pointerDown);
    };
  }, [onClose, onDismiss]);
  return (
    <div ref={ref} className="command-utility-popover" role="dialog" aria-modal="false"
      aria-labelledby={titleId} data-command-name={commandName(item)}
      style={{ '--command-surface': t.surface, '--command-surface-muted': t.surfaceHover,
        '--command-text': t.text, '--command-text-muted': t.text3, '--command-ring': t.border,
        '--command-accent': item.isError ? t.danger : commandColor, '--command-danger': t.danger,
        '--command-identity-color': commandColor, '--command-available-height': `${maxHeight}px`,
      } as CSSProperties}>
      <header className="command-utility-header">
        <CommandIcon command={item.name} size={28} />
        <div className="command-utility-heading">
          <h2 id={titleId} data-error={item.isError || undefined}>
            {item.isError && <Icon name="shieldAlert" size={17} />}{presentation.title}
          </h2>
          <p>{item.name || tr("slash_command_output_title")}</p>
        </div>
        <button type="button" aria-label={tr("desktop_ui_close_command")} onClick={onClose}><Icon name="x" size={18} /></button>
      </header>
      <div className="command-utility-content" role={item.isError ? 'alert' : undefined} aria-live={item.isError ? undefined : 'polite'} data-error={item.isError || undefined}>
        <CommandBody item={item} />
      </div>
      <footer className="command-utility-footer">{tr('slash_result_temporary')}</footer>
    </div>
  );
}
