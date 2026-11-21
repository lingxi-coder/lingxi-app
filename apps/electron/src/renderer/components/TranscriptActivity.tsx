import type { CSSProperties } from 'react';
import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { useT } from '../theme/ThemeContext';
import { Disclosure } from './Disclosure';
import { ToolCall, toolIconName } from './ToolCall';
import { ToolActivityIcon } from './ToolActivityIcon';
import { MarkdownContent } from './MarkdownContent';
import { ApiRetryNotice, type ApiRetryStatus } from './ApiRetryNotice';
import { toolGroupSummary } from './toolPresentation';
import type { TranscriptActivity as Activity } from './transcriptActivities';
import './TranscriptActivity.css';
import { TranscriptLeafWindow } from './TranscriptLeafWindow';
import { LocalAppInstructions } from './LocalAppInstructions';

export function TranscriptActivity({ activity, open, toolOpen, onSetOpen, modUiSessionId, retry }: {
  activity: Activity;
  open: boolean;
  toolOpen(id: string): boolean | undefined;
  onSetOpen(id: string, next: boolean): void;
  modUiSessionId?: string;
  retry?: ApiRetryStatus;
}) {
  const t = useT();
  useI18n();
  const running = activity.tools.some(tool => tool.status === 'running');
  const label = activity.skill?.name
    || (activity.tools.length ? toolGroupSummary(activity.tools) : tr(activity.active ? 'chat_thinking' : 'desktop_activity'));
  const status = running ? tr('chat_status_running')
    : activity.active && activity.tools.length ? tr('chat_thinking')
    : activity.tools.length && activity.tools.every(tool => tool.status === 'error') ? tr('chat_status_failed') : '';
  const first = activity.tools[0];
  // A lone tool owns its details; another disclosure only repeats its header.
  if (activity.tools.length === 1 && first && !activity.thoughts.some(thought => thought.text.trim()) && !retry) {
    return <ToolCall item={first} modUiSessionId={modUiSessionId}
      open={toolOpen(first.id)} onSetOpen={onSetOpen} />;
  }
  if (retry) {
    return <ApiRetryNotice retry={retry}>
      {(activity.tools.length > 0 || activity.thoughts.some(thought => thought.text.trim())) &&
        <div className="transcript-activity-details" style={{ '--activity-border': t.border } as CSSProperties}>
          <TranscriptLeafWindow items={[...activity.tools, ...activity.thoughts.filter(thought => thought.text.trim())]}
            renderItem={item => item.type === 'tool'
              ? <ToolCall item={item} modUiSessionId={modUiSessionId} open={toolOpen(item.id)} onSetOpen={onSetOpen} />
              : <MarkdownContent text={item.text} />} />
        </div>}
    </ApiRetryNotice>;
  }
  return <div className="transcript-activity" data-activity-id={activity.id} data-active={activity.active || undefined}
    style={{ '--activity-text': t.text3, '--activity-highlight': t.text, '--activity-ring': t.accent,
      '--activity-border': t.border } as CSSProperties}>
    <Disclosure id={activity.id} open={open} onToggle={() => onSetOpen(activity.id, !open)}
      buttonClassName="transcript-activity-trigger" buttonStyle={{ fontSize: 13, lineHeight: '20px' }} label={[label, status].filter(Boolean).join(' · ')}
      summary={<>
        <ToolActivityIcon name={first ? toolIconName(first.view.verb, first.tool, first.view.icon) : 'spark'} />
        <span className={`transcript-activity-label${activity.active ? ' running-sweep' : ''}`}
          role={activity.active && !status ? 'status' : undefined} title={label}
          style={{ '--sweep-base': t.text3, '--sweep-highlight': t.text } as CSSProperties}>{label}</span>
        <span className="transcript-activity-state" role="status">{status}</span>
      </>}>
      <div className="transcript-activity-details">
        <TranscriptLeafWindow items={activity.skill?.items ?? [...activity.tools, ...activity.thoughts.filter(thought => thought.text.trim())]}
          renderItem={item => {
            if (item.type === 'tool') return <ToolCall item={item} modUiSessionId={modUiSessionId} open={toolOpen(item.id)} onSetOpen={onSetOpen} />;
            if (item.type === 'skill-instructions') return <LocalAppInstructions id={item.id} name={item.name} text={item.text} open={toolOpen(item.id) ?? false} onSetOpen={onSetOpen} />;
            return item.text.trim() ? <MarkdownContent text={item.text} /> : null;
          }} />
      </div>
    </Disclosure>
  </div>;
}
