/** The centre of the workspace while there is nothing to show: loading, or why a link cannot open. */
export function PaneNotice({ title, text, busy, action }: { title?: string; text: string; busy?: boolean; action?: { href: string; label: string } }) {
  return <div className="chat-notice" role={busy ? 'status' : undefined}>
    {title ? <h2>{title}</h2> : null}
    <p>{text}</p>
    {action ? <a className="chat-button" href={action.href}>{action.label}</a> : null}
  </div>;
}
