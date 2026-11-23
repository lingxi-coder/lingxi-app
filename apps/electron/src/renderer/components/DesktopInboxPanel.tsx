import { useEffect, useState } from 'react';
import { InboxList } from '../../../../../packages/inbox/InboxList';
import type { InboxItem, InboxNoticeTarget } from '../../../../../packages/inbox/contracts';
import { tr } from '../i18n/locale';
import type { useDesktopInbox } from '../bridge/useDesktopInbox';
import './DesktopInboxPanel.css';

export function DesktopInboxPanel({ inbox, locale, activeKey, onOpen }: {
  inbox: ReturnType<typeof useDesktopInbox>; locale: string;
  activeKey?: string;
  onOpen(item: InboxItem, target?: InboxNoticeTarget): Promise<void>;
}) {
  const cn = locale.startsWith('zh');
  const [preferences, setPreferences] = useState<{ uid: string; sync: boolean; receipts: boolean }>();
  const [conversationSync, setConversationSync] = useState<{ uid: string; key: string; enabled: boolean }>();
  const [busy, setBusy] = useState(false), [failed, setFailed] = useState(false);
  useEffect(() => {
    if (!inbox.api || !inbox.uid) { setPreferences(undefined); return; }
    let active = true;
    const uid = inbox.uid;
    void Promise.all([inbox.api.summarySync(uid), inbox.api.request<{ dmReceipts: boolean }>(uid, { action: 'accountPreferences' })])
      .then(([sync, value]) => { if (active) setPreferences({ uid, sync, receipts: value.dmReceipts }); })
      .catch(() => { if (active) setFailed(true); });
    return () => { active = false; };
  }, [inbox.api, inbox.uid]);
  useEffect(() => {
    setConversationSync(undefined);
    if (!inbox.api || !inbox.uid || !activeKey?.startsWith('assistant:')) return;
    let current = true;
    const api = inbox.api, uid = inbox.uid, key = activeKey;
    void api.localRef(key).then(async ref => {
      if (!ref || !current) return;
      const enabled = await api.conversationSummarySync(uid, key);
      if (current) setConversationSync({ uid, key, enabled });
    }).catch(() => { if (current) setFailed(true); });
    return () => { current = false; };
  }, [inbox.api, inbox.uid, activeKey]);
  const save = async (kind: 'sync' | 'receipts', enabled: boolean) => {
    if (!inbox.api || !inbox.uid || busy) return;
    const uid = inbox.uid;
    setBusy(true); setFailed(false);
    try {
      if (kind === 'sync') await inbox.api.setSummarySync(uid, enabled);
      else await inbox.api.request(uid, { action: 'accountPreferences', dmReceipts: enabled });
      setPreferences(current => current?.uid === uid ? { ...current, [kind]: enabled } : current);
    } catch { setFailed(true); }
    finally { setBusy(false); }
  };
  const saveConversationSync = async (enabled: boolean) => {
    if (!inbox.api || !inbox.uid || !conversationSync || busy) return;
    const { uid, key } = conversationSync;
    setBusy(true); setFailed(false);
    try {
      await inbox.api.setConversationSummarySync(uid, key, enabled);
      setConversationSync(current => current?.uid === uid && current.key === key ? { ...current, enabled } : current);
    } catch { setFailed(true); }
    finally { setBusy(false); }
  };
  if (!inbox.controller) return null;
  const settings = inbox.uid ? (
    <details className="lx-inbox-settings desktop-inbox-preferences">
      <summary>{cn ? '会话与隐私设置' : 'Conversation settings'}</summary>
      <label><input type="checkbox" disabled={busy || preferences?.uid !== inbox.uid} checked={preferences?.uid === inbox.uid && preferences.sync}
        onChange={event => { void save('sync', event.target.checked); }} />{cn ? '跨设备同步会话摘要与已读' : 'Sync summaries and reading position'}</label>
      <p>{cn ? '同步已命名会话的标题、设备、状态和阅读位置。助手正文保留在原设备，远程访问单独授权。' : 'Sync named titles, device, status and reading position. Assistant history stays on its source device; remote access is authorized separately.'}</p>
      {conversationSync?.uid === inbox.uid && conversationSync.key === activeKey ? <label>
        <input type="checkbox" disabled={busy || preferences?.uid !== inbox.uid || !preferences.sync} checked={conversationSync.enabled}
          onChange={event => { void saveConversationSync(event.target.checked); }} />
        {cn ? '同步当前助手会话的摘要' : 'Sync this assistant conversation’s summary'}
      </label> : null}
      <label><input type="checkbox" disabled={busy || preferences?.uid !== inbox.uid} checked={preferences?.uid === inbox.uid && preferences.receipts}
        onChange={event => { void save('receipts', event.target.checked); }} />{cn ? '分享私聊已读回执' : 'Share direct-message read receipts'}</label>
      <p>{cn ? '双方开启后才显示对方已读。此设置不影响自己的多端阅读同步。' : 'Receipts appear only when both people enable them. Your own reading position always syncs.'}</p>
      {failed ? <p role="alert">{cn ? '暂时无法保存或读取设置，请稍后重试。' : 'Settings are temporarily unavailable. Please try again.'}</p> : null}
    </details>
  ) : null;
  return <div className="desktop-inbox-panel">
    <InboxList controller={inbox.controller} locale={locale} activeKey={activeKey} onOpen={onOpen}
      heading={<h2>{tr('tw_messages')}</h2>} settings={settings} />
    {!inbox.uid && <p className="desktop-inbox-sign-in">{cn ? '登录后查看好友、团队和 Bot 消息。' : 'Sign in to view messages from friends, teams, and bots.'}</p>}
  </div>;
}
