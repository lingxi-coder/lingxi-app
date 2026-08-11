/**
 * A CONTROLLED disclosure — one button, one body, no state of its own.
 *
 * Three ad-hoc toggles had grown in the transcript, each with its own subset of
 * the accessibility contract and each holding `open` in row-local `useState`.
 * Row-local state is the bug: every list here recycles rows, so a disclosure's
 * open/closed state was silently reassigned to a different item whenever one
 * was inserted above it. This component therefore takes `open` and `onToggle`
 * from the owner, which keeps the state in the model/store layer where the
 * item's stable id can key it.
 *
 * Deliberately NOT animated. A height transition on a transcript that repaints
 * on every streaming delta costs a layout pass per frame for a 150ms flourish,
 * and the body is UNMOUNTED when closed — a collapsed tool call is ~6 DOM
 * nodes, which is the entire reason this list survives without a virtualizer.
 */

import type { CSSProperties, ReactNode } from 'react';
import { Icon } from './Icon';
import { useT } from '../theme/ThemeContext';

export interface DisclosureProps {
  /**
   * Stable, unique id — becomes the body's DOM id and the button's
   * `aria-controls`. Must come from the item's model id, never an array index.
   */
  id: string;
  /** Whether the body is shown. Owned by the caller. */
  open: boolean;
  /** Toggle request. The caller decides what "open" means for this id. */
  onToggle(): void;
  /** Summary content rendered inside the button, after the chevron. */
  summary: ReactNode;
  /** Body content. Rendered only while {@link open}. */
  children: ReactNode;
  /** Accessible name when {@link summary} is not plain text. */
  label?: string;
  /** Extra styles merged onto the trigger button. */
  buttonStyle?: CSSProperties;
  /** Extra styles merged onto the body wrapper. */
  bodyStyle?: CSSProperties;
}

const TRIGGER_BASE: CSSProperties = {
  display: 'inline-flex',
  alignItems: 'center',
  gap: 6,
  alignSelf: 'flex-start',
  border: 'none',
  padding: 0,
  background: 'transparent',
  font: 'inherit',
  textAlign: 'left',
  cursor: 'pointer',
};

export function Disclosure({
  id,
  open,
  onToggle,
  summary,
  children,
  label,
  buttonStyle,
  bodyStyle,
}: DisclosureProps) {
  const t = useT();
  const bodyId = `disclosure-body-${id}`;
  return (
    <>
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        aria-controls={bodyId}
        aria-label={label}
        style={{ color: t.text3, ...TRIGGER_BASE, ...buttonStyle }}
      >
        <Icon name={open ? 'chevron' : 'chevronR'} size={13} stroke={2} />
        {summary}
      </button>
      {open && (
        <div id={bodyId} style={bodyStyle}>
          {children}
        </div>
      )}
    </>
  );
}
