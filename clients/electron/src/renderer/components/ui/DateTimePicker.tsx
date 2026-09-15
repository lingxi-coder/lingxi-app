import { useId, useLayoutEffect, useRef, useState, type CSSProperties } from 'react';
import { useT } from '../../theme/ThemeContext';
import './DateTimePicker.css';

type Props = {
  mode: 'date' | 'time' | 'datetime-local';
  value: string;
  onChange: (value: string) => void;
  label: string;
  disabled?: boolean;
};
const pad = (value: number) => String(value).padStart(2, '0');
const dateValue = (date: Date) => `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;

/** Shared desktop picker. Values stay in local wall-clock time, without UTC conversion. */
export function DateTimePicker({ mode, value, onChange, label, disabled }: Props) {
  const t = useT();
  const id = useId();
  const dialog = useRef<HTMLDialogElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const [month, setMonth] = useState(() => new Date());
  const [hours, setHours] = useState('00');
  const [minutes, setMinutes] = useState('00');
  const [position, setPosition] = useState({ left: 0, top: 0 });
  const hasDate = mode !== 'time';
  const hasTime = mode !== 'date';
  const selectedDate = hasDate && value ? new Date(`${value.slice(0, 10)}T12:00:00`) : new Date();
  const safeDate = Number.isNaN(selectedDate.getTime()) ? new Date() : selectedDate;
  const selectedTime = mode === 'time' ? value : value.slice(11, 16);
  const theme = {
    '--picker-bg': t.surface, '--picker-text': t.text, '--picker-secondary': t.text3,
    '--picker-fill': t.surfaceHover, '--picker-border': t.border, '--picker-accent': t.accent,
    colorScheme: t.dark ? 'dark' : 'light',
  } as CSSProperties;

  useLayoutEffect(() => {
    if (!open) return;
    const reposition = () => {
      const anchor = trigger.current!.getBoundingClientRect();
      const panel = dialog.current!.getBoundingClientRect();
      setPosition({
        left: Math.max(12, Math.min(anchor.right - panel.width, window.innerWidth - panel.width - 12)),
        top: Math.max(12, Math.min(anchor.bottom + 8, window.innerHeight - panel.height - 12)),
      });
    };
    reposition();
    window.addEventListener('resize', reposition);
    return () => window.removeEventListener('resize', reposition);
  }, [open, month, hasTime]);

  function show() {
    setMonth(new Date(safeDate.getFullYear(), safeDate.getMonth(), 1));
    setHours(selectedTime.slice(0, 2) || '00');
    setMinutes(selectedTime.slice(3, 5) || '00');
    setOpen(true);
    dialog.current!.showModal();
  }
  function selectDate(date: Date) {
    onChange(dateValue(date) + (hasTime ? `T${selectedTime || '00:00'}` : ''));
  }
  function changeTime(part: 'hour' | 'minute', next: string) {
    if (part === 'hour') setHours(next); else setMinutes(next);
    const h = part === 'hour' ? next : hours;
    const m = part === 'minute' ? next : minutes;
    if (!/^\d{1,2}$/.test(h) || !/^\d{1,2}$/.test(m) || +h > 23 || +m > 59) return;
    const time = `${pad(+h)}:${pad(+m)}`;
    onChange(hasDate ? `${dateValue(safeDate)}T${time}` : time);
  }
  function close() {
    if (!Array.from(dialog.current!.querySelectorAll('input')).every((input) => input.reportValidity())) return;
    dialog.current!.close();
  }
  const days = new Date(month.getFullYear(), month.getMonth() + 1, 0).getDate();
  const weekdayNames = Array.from({ length: 7 }, (_, index) => new Date(2024, 0, 7 + index).toLocaleDateString(undefined, { weekday: 'narrow' }));
  return <span className="lx-date-picker" style={theme}>
    <button ref={trigger} type="button" className="lx-date-trigger" aria-label={label} aria-describedby={`${id}-value`} aria-haspopup="dialog" aria-expanded={open} aria-controls={id} disabled={disabled} onClick={show}>
      <span id={`${id}-value`} className="lx-date-value">
      {hasDate && <span>{value ? safeDate.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' }) : 'Choose date'}</span>}
      {hasTime && <span>{selectedTime || 'Choose time'}</span>}
      </span>
    </button>
    <dialog ref={dialog} id={id} className="lx-date-dialog" aria-label={label} style={{ ...theme, ...position }} onKeyDown={(event) => {
      if (event.key === 'Enter' && event.target instanceof HTMLInputElement) {
        event.preventDefault(); event.stopPropagation(); close();
      }
    }} onClose={() => { setOpen(false); trigger.current?.focus(); }} onClick={(event) => {
      if (event.target === event.currentTarget) { const rect = event.currentTarget.getBoundingClientRect(); if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) dialog.current?.close(); }
    }}>
      <header className="lx-date-heading"><span>{label}</span><button type="button" onClick={close}>Done</button></header>
      {hasDate && <>
        <nav className="lx-date-month" aria-label="Calendar navigation">
          <span aria-live="polite">{month.toLocaleDateString(undefined, { month: 'long', year: 'numeric' })}</span>
          <button type="button" aria-label="Previous month" onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() - 1, 1))}>‹</button>
          <button type="button" aria-label="Next month" onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() + 1, 1))}>›</button>
        </nav>
        <div className="lx-date-calendar">
          {weekdayNames.map((name, index) => <span className="lx-date-weekday" key={index}>{name}</span>)}
          {Array.from({ length: month.getDay() }, (_, index) => <span key={`empty-${index}`} />)}
          {Array.from({ length: days }, (_, index) => {
            const date = new Date(month.getFullYear(), month.getMonth(), index + 1);
            const selected = dateValue(date) === value.slice(0, 10);
            return <button type="button" key={index} tabIndex={selected || (index === 0 && (safeDate.getMonth() !== month.getMonth() || safeDate.getFullYear() !== month.getFullYear() || !value)) ? 0 : -1} aria-label={dateValue(date)} aria-pressed={selected} aria-current={dateValue(date) === dateValue(new Date()) ? 'date' : undefined}
              onClick={() => selectDate(date)} onKeyDown={(event) => {
                const offset = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -7, ArrowDown: 7 }[event.key];
                if (offset === undefined) return;
                event.preventDefault();
                const next = new Date(date.getFullYear(), date.getMonth(), date.getDate() + offset);
                setMonth(new Date(next.getFullYear(), next.getMonth(), 1)); selectDate(next);
                requestAnimationFrame(() => dialog.current?.querySelector<HTMLButtonElement>(`[aria-label="${dateValue(next)}"]`)?.focus());
              }}>{index + 1}</button>;
          })}
        </div>
      </>}
      {hasTime && <div className="lx-date-time"><span>Time <small>24h</small></span><div className="lx-date-segments">
        <input aria-label="Hour" type="number" min="0" max="23" required value={hours} onChange={(event) => changeTime('hour', event.target.value)} onFocus={(event) => event.target.select()} onBlur={() => setHours(selectedTime.slice(0, 2) || '00')} />
        <span aria-hidden="true">:</span>
        <input aria-label="Minute" type="number" min="0" max="59" required value={minutes} onChange={(event) => changeTime('minute', event.target.value)} onFocus={(event) => event.target.select()} onBlur={() => setMinutes(selectedTime.slice(3, 5) || '00')} />
      </div></div>}
    </dialog>
  </span>;
}
