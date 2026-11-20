import { useEffect } from 'react';

/** The centre of the workspace while there is nothing to show: loading, why a link cannot open, or a failed load to retry. */
export function PaneNotice({ title, text, busy, action, retry }: { title?: string; text: string; busy?: boolean; action?: { href: string; label: string }; retry?: { label: string; run(): void } }) {
  // A failed load also tries again by itself when the connection or the tab comes back.
  useEffect(() => {
    if (!retry) return;
    const again = () => retry.run();
    window.addEventListener('online', again); window.addEventListener('focus', again);
    return () => { window.removeEventListener('online', again); window.removeEventListener('focus', again); };
  }, [retry]);
  return <div className="chat-notice" role={busy ? 'status' : retry ? 'alert' : undefined}>
    {title ? <h2>{title}</h2> : null}
    <p>{text}</p>
    {action ? <a className="chat-button" href={action.href}>{action.label}</a> : null}
    {retry ? <button type="button" className="chat-button" onClick={retry.run}>{retry.label}</button> : null}
  </div>;
}
