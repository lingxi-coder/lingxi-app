import { useContext, useId, type CSSProperties } from 'react';
import { AccountSignInContext } from '../bridge/useAccountState';
import { tr } from '../i18n/locale';
import { useT } from '../theme/ThemeContext';

/** What is said, and offered, where there is no signed-in account: the sidebar's short form, or the page's. */
export function TeamsSignIn({ compact = false }: { compact?: boolean }) {
  const t = useT();
  const requestSignIn = useContext(AccountSignInContext);
  // The sidebar and the page can both say it at once, so each names its own heading.
  const heading = useId();
  return (
    <section aria-labelledby={heading} style={compact ? { padding: '3px 4px' } : pageStyle}>
      {compact
        ? <h2 id={heading} style={{ minHeight: 35, padding: '3px 4px 5px 8px', display: 'flex', alignItems: 'center', color: t.text4, fontSize: 12.5, fontWeight: 500, letterSpacing: '.01em' }}>{tr('teams_title')}</h2>
        : <h2 id={heading} style={{ margin: 0, color: t.text, fontSize: 20, fontWeight: 650, letterSpacing: '-.02em' }}>{tr('desktop_teams_sign_in_title')}</h2>}
      <p style={{ ...noteStyle, color: compact ? t.text4 : t.text3, fontSize: compact ? 11.5 : 14, padding: compact ? '0 10px 8px 9px' : 0, maxWidth: 360 }}>{tr('desktop_teams_sign_in_body')}</p>
      {requestSignIn
        ? <button type="button" onClick={requestSignIn}
          style={{ marginLeft: compact ? 9 : 0, minHeight: 32, padding: '5px 14px', border: `0.5px solid ${t.border}`, borderRadius: 8, background: 'transparent', color: t.text, cursor: 'pointer', fontSize: 12.5, fontWeight: 500 }}>
          {tr('desktop_shell_sign_in')}
        </button>
        : null}
    </section>
  );
}

/** A line of plain text in the sidebar or on the page: the sign-in note, or that the workspace is loading. */
export const noteStyle: CSSProperties = { margin: 0, lineHeight: 1.5, fontSize: 11.5, padding: '8px 10px 8px 9px' };
const pageStyle: CSSProperties = { margin: 'auto', padding: 28, display: 'grid', justifyItems: 'start', gap: 12 };
