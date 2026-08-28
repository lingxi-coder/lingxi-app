import type { useT } from '../../../theme/ThemeContext';

/**
 * Shared inline "ghost" button chrome for the client-owned settings pages
 * (Diagnostics' copy/export/refresh/restart actions, Projects' add/switch/
 * remove/unpin actions). `rows.tsx` deliberately does not cover button
 * chrome — `Card`/`Row` are layout only — so this is page-local styling,
 * not a competing layout primitive. Pulled out once both pages had grown
 * their own near-identical copy of it.
 */
export function ghostButtonStyle(t: ReturnType<typeof useT>, disabled = false, danger = false) {
  return {
    padding: '6px 12px', borderRadius: 7, border: `0.5px solid ${t.border}`, fontFamily: 'inherit',
    background: disabled ? t.surfaceActive : t.surface, cursor: disabled ? 'not-allowed' : 'pointer',
    color: disabled ? t.text4 : danger ? t.danger : t.text2,
    fontSize: 12, fontWeight: 500, display: 'inline-flex', alignItems: 'center', gap: 5,
  } as const;
}
