import { useLayoutEffect, useRef, type RefObject } from 'react';

/** Paint a hint at the draft's end without adding text to the editable DOM. */
export function ComposerSlashArgumentHint({ editor, text, hint, color }: {
  editor: RefObject<HTMLDivElement>;
  text: string;
  hint: string;
  color: string;
}) {
  const overlay = useRef<HTMLSpanElement>(null);

  useLayoutEffect(() => {
    const input = editor.current;
    const target = overlay.current;
    const container = target?.parentElement;
    if (!input || !target || !container) return;
    const measure = () => {
      // Hint eligibility requires a trailing space, so the final editable leaf
      // is a text node even when contentEditable has inserted block elements.
      let last: Node = input;
      while (last.lastChild) last = last.lastChild;
      const range = document.createRange();
      range.selectNodeContents(last);
      range.collapse(false);
      const end = range.getBoundingClientRect();
      const bounds = input.getBoundingClientRect();
      const parent = container.getBoundingClientRect();
      const style = getComputedStyle(input);
      const available = input.clientWidth - parseFloat(style.paddingRight) - (end.left - bounds.left);
      target.style.visibility = end.height > 0 && available > 0
        && end.top >= bounds.top && end.bottom <= bounds.bottom ? 'visible' : 'hidden';
      Object.assign(target.style, {
        left: `${end.left - parent.left}px`, top: `${end.top - parent.top}px`,
        width: `${Math.max(0, available)}px`, height: `${end.height}px`,
        fontFamily: style.fontFamily, fontSize: style.fontSize, fontWeight: style.fontWeight,
        fontStyle: style.fontStyle, letterSpacing: style.letterSpacing,
        lineHeight: `${end.height}px`,
      });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(input);
    input.addEventListener('scroll', measure, { passive: true });
    return () => { observer.disconnect(); input.removeEventListener('scroll', measure); };
  }, [editor, text, hint]);

  return <span ref={overlay} id="composer-slash-argument-hint" className="composer-slash-argument-hint" aria-hidden="true" style={{ color }}>{hint}</span>;
}
