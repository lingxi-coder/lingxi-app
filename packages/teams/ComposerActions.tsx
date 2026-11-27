import { useEffect, useId, useRef, useState, type KeyboardEvent } from 'react';
import { useTeamsText } from './i18n.js';
import { Icon, nextEnabled, useIsoLayoutEffect } from './ui';
import { messageStyles } from './message-format.js';

export type FormatAction = keyof typeof messageStyles;
const actions = [
  ['bold', 'B', 'tw_format_bold'], ['italic', 'I', 'tw_format_italic'],
  ['underline', 'U', 'tw_format_underline'], ['strikeThrough', 'S', 'tw_format_strike'],
] as const;

export function ComposerActions({ locale, onOpen, onAttach, onFormat, activeStyles }: {
  locale: string;
  onOpen(): void;
  onAttach?: (kind: 'photos' | 'files') => void;
  onFormat(action: FormatAction): void;
  activeStyles: number;
}) {
  const t = useTeamsText(locale);
  const id = useId();
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const [menu, setMenu] = useState<'add' | 'format'>();
  const items = () => Array.from(root.current?.querySelectorAll<HTMLButtonElement>('button[role^="menuitem"]') ?? []);

  useIsoLayoutEffect(() => {
    if (menu) root.current?.querySelector<HTMLButtonElement>('button[role^="menuitem"]')?.focus();
  }, [menu]);

  useEffect(() => {
    if (!menu || !root.current) return;
    const document = root.current.ownerDocument;
    const dismiss = (event: Event) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setMenu(undefined);
    };
    document.addEventListener('pointerdown', dismiss, true);
    document.addEventListener('focusin', dismiss);
    return () => {
      document.removeEventListener('pointerdown', dismiss, true);
      document.removeEventListener('focusin', dismiss);
    };
  }, [menu]);

  const open = () => {
    onOpen();
    setMenu(onAttach ? 'add' : 'format');
  };

  const keys = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!menu) return;
    const buttons = items(), index = buttons.indexOf(event.target as HTMLButtonElement);
    const direction = event.key === 'ArrowDown' ? 1 : -1;
    switch (event.key) {
      case 'ArrowDown': case 'ArrowUp':
        buttons[nextEnabled(buttons, index < 0 && direction < 0 ? 0 : index, direction)]?.focus();
        break;
      case 'Home': buttons[0]?.focus(); break;
      case 'End': buttons.at(-1)?.focus(); break;
      case 'ArrowRight':
        if (menu !== 'add' || buttons[index]?.getAttribute('aria-haspopup') !== 'menu') return;
        setMenu('format');
        break;
      case 'ArrowLeft':
        if (menu !== 'format' || !onAttach) return;
        setMenu('add');
        break;
      case 'Escape':
        setMenu(undefined);
        trigger.current?.focus();
        break;
      case 'Tab':
        trigger.current?.focus();
        setMenu(undefined);
        return;
      default: return;
    }
    event.preventDefault();
    event.stopPropagation();
  };

  const attach = (kind: 'photos' | 'files') => {
    setMenu(undefined);
    trigger.current?.focus();
    onAttach?.(kind);
  };
  const label = t('tw_composer_add');
  const format = t('tw_composer_format');
  return <div ref={root} className="tw-compose-actions" onKeyDown={keys}>
    <button ref={trigger} type="button" className="tw-icon-btn tw-compose-add" aria-label={label} title={label} aria-haspopup="menu"
      aria-expanded={!!menu} aria-controls={menu ? id : undefined}
      onMouseDown={event => event.preventDefault()}
      onKeyDown={event => {
        if (!menu && event.key === 'ArrowDown') { event.preventDefault(); open(); }
      }}
      onClick={() => { if (menu) { setMenu(undefined); trigger.current?.focus(); } else open(); }}><Icon name="plus" size={19} /></button>
    {menu ? <div id={id} role="menu" aria-label={menu === 'add' ? label : format} className="tw-compose-menu" data-page={menu} key={menu}>
      {menu === 'add' ? <>
        <button type="button" role="menuitem" tabIndex={-1} onClick={() => attach('photos')}>
          <span className="tw-compose-menu-icon"><Icon name="image" size={17} /></span>{t('teams_file_add_photos')}
        </button>
        <button type="button" role="menuitem" tabIndex={-1} title={t('tw_composer_attach')}
          onClick={() => attach('files')}>
          <span className="tw-compose-menu-icon"><Icon name="clip" size={17} /></span>{t('teams_file_add')}
        </button>
        <button type="button" role="menuitem" tabIndex={-1} aria-haspopup="menu" onClick={() => setMenu('format')}>
          <span className="tw-compose-menu-icon tw-compose-menu-letters" aria-hidden="true">Aa</span>{format}
          <span className="tw-compose-menu-next" aria-hidden="true">›</span>
        </button>
      </> : <>
        {onAttach ? <button type="button" role="menuitem" tabIndex={-1} className="tw-compose-menu-back" onClick={() => setMenu('add')}>
          <span aria-hidden="true">‹</span>{t('remote_back')}
        </button> : null}
        {actions.map(([action, glyph, title]) => {
          const checked = Boolean(activeStyles & messageStyles[action]);
          return <button key={action} type="button" role="menuitemcheckbox" aria-checked={checked} tabIndex={-1}
            onClick={() => { setMenu(undefined); onFormat(action); }}>
            <span className="tw-compose-menu-icon tw-compose-format-symbol" data-style={action} aria-hidden="true">{glyph}</span>{t(title)}
            {checked ? <Icon name="check" size={16} /> : null}
          </button>;
        })}
      </>}
    </div> : null}
  </div>;
}
