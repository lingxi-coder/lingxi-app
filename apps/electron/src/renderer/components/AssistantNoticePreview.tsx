import { useEffect, useState } from 'react';
import type { MessageDto } from '@lingxi/bridge-client';
import type { InboxNoticeTarget } from '../../../../../packages/inbox/contracts';
import type { DesktopInboxApi } from '../../shared/inbox';
import { MarkdownContent } from './MarkdownContent';

export function AssistantNoticePreview({ api, target, locale, onReturn }: {
  api: DesktopInboxApi;
  target: InboxNoticeTarget & { key: string };
  locale: string;
  onReturn(): void;
}) {
  const cn = locale.startsWith('zh');
  const [result, setResult] = useState<{ available: boolean; messages: MessageDto[] }>();
  useEffect(() => {
    let current = true;
    setResult(undefined);
    if (target.through < 1) setResult({ available: false, messages: [] });
    else void api.activityPreview(target.key, target.through)
      .then(value => { if (current) setResult(value); })
      .catch(() => { if (current) setResult({ available: false, messages: [] }); });
    return () => { current = false; };
  }, [api, target.key, target.through]);
  return <section className="desktop-assistant-notice-preview" aria-label={cn ? '通知预览' : 'Notification preview'}>
    <header><h2>{cn ? '会话通知' : 'Conversation notification'}</h2>
      <button type="button" onClick={onReturn}>{cn ? '返回会话' : 'Return to conversation'}</button></header>
    {result === undefined ? <p role="status">{cn ? '正在读取…' : 'Loading…'}</p>
      : !result.available ? <p role="status">{cn ? '此通知没有可显示的结果。请返回会话查看当前状态。' : 'This notification has no available result. Return to the conversation to see its current status.'}</p>
        : result.messages.map((message, index) => <article key={message.transcript_uuid ?? index}>
          {message.blocks.map((block, blockIndex) => block.type === 'text'
            ? <MarkdownContent key={blockIndex} text={block.text} trustedHtml={false} botProposals={false} externalLinksOnly /> : null)}
        </article>)}
  </section>;
}
