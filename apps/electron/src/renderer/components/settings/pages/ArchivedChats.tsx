import { useState } from 'react';
import type { ArchivedSessionRecord } from '../../../../shared/settings';
import type { PageContentProps } from '../SettingsScreen';
import { Card } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { ghostButtonStyle, inputStyle } from './ghostButton';

export function archivedChatRows(records: readonly ArchivedSessionRecord[], query: string) {
  const needle = query.trim().toLocaleLowerCase();
  return records.filter((row) => !needle || [row.title, row.projectPath, row.sessionId].some((value) => value?.toLocaleLowerCase().includes(needle)))
    .slice().sort((a, b) => (b.archivedAt ?? '').localeCompare(a.archivedAt ?? ''));
}

export function ArchivedChats({ bridge, onClose }: PageContentProps) {
  const t = useT();
  const [query, setQuery] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState('');
  const records = bridge.bootstrap?.settings.archivedSessions ?? [];
  const rows = archivedChatRows(records, query);
  const restore = async (row: ArchivedSessionRecord) => {
    if (busy) return;
    setBusy(JSON.stringify([row.projectPath, row.sessionId]));
    setError('');
    try { await bridge.openSession(row.projectPath, row.sessionId); onClose(); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(null); }
  };
  return <Card title={`Archived chats (${records.length})`}>
    <div style={{ padding: 16 }}>
    <p style={{ color: t.text3, fontSize: 13 }}>显示所有项目的已归档会话。恢复后会重新出现在侧栏。</p>
    <input aria-label="搜索已归档会话" placeholder="搜索标题或项目…" value={query} onChange={(event) => setQuery(event.target.value)} style={{ ...inputStyle(t), width: '100%', boxSizing: 'border-box', marginBottom: 12 }} />
    {error && <p role="alert" style={{ color: t.danger }}>{error}</p>}
    {!rows.length && <p role="status" style={{ color: t.text3, fontSize: 13 }}>{records.length ? '没有匹配的会话。' : '暂无已归档会话。'}</p>}
    <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
      {rows.map((row) => {
        const key = JSON.stringify([row.projectPath, row.sessionId]);
        const date = row.archivedAt ? new Date(row.archivedAt) : null;
        return <li key={key} style={{ display: 'flex', alignItems: 'center', gap: 12, padding: '12px 0', borderTop: `1px solid ${t.border}` }}>
          <div style={{ flex: 1, minWidth: 0, overflowWrap: 'anywhere' }}>
            <div style={{ color: t.text, fontSize: 14 }}>{row.title || '未命名会话'}</div>
            <div style={{ color: t.text3, fontSize: 12, marginTop: 4 }}>{row.projectPath}</div>
            {date && !Number.isNaN(date.getTime()) && <time dateTime={row.archivedAt} style={{ color: t.text3, fontSize: 12 }}>{date.toLocaleString()}</time>}
          </div>
          <button type="button" disabled={busy !== null} aria-label={`恢复并打开 ${row.title || '未命名会话'}`} onClick={() => void restore(row)} style={{ ...ghostButtonStyle(t, busy !== null), flexShrink: 0 }}>{busy === key ? '恢复中…' : '恢复并打开'}</button>
        </li>;
      })}
    </ul>
    </div>
  </Card>;
}
