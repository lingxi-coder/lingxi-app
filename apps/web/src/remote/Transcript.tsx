import { useMemo, useState } from 'react';
import type { SessionAgentSummaryDto, StructuredDiffDto } from '@lingxi/bridge-client';
import type { Locale } from '../utils/locale';
import { Markdown } from './markdown';
import { imageRefOf, itemsWithExtras, type ExtraRow, type PromptMeta, type Transcript } from './sessionModel';
import { AGENT_AVATARS, agentAvatarIndex, anchorTranscriptAgents, placeTranscriptAgents, isTaskProgressTool, isTaskWorkflowTool, transcriptRows, toolDisplayHeader, toolHasBody, toolTruncationNotice, type RunItem, type ToolRunItem, type AgentTranscriptRow } from './reuse';
import { t } from './strings';
import { displayText } from './text';

interface TranscriptViewProps {
  locale: Locale;
  transcript: Transcript;
  running: boolean;
  /** This browser's device name, to tell its own prompts from other devices'. */
  deviceName: string;
  fetchImage(ref: string): Promise<string>;
}

function describeExtra(locale: Locale, extra: ExtraRow): string {
  if (extra.kind === 'omitted') return extra.detail ? t(locale, 'omittedKind', { kind: extra.detail }) : t(locale, 'omittedNote');
  return extra.detail ? t(locale, 'computerAccessNote', { apps: extra.detail }) : t(locale, 'computerAccessNoteNoApps');
}

function originLabel(locale: Locale, meta: PromptMeta, thisDevice: string): string {
  if (meta.origin === 'desktop') return t(locale, 'fromDesktop');
  if (meta.origin === 'scheduled') return t(locale, 'fromScheduled');
  if (meta.origin === 'bot') return meta.fromBotName ? t(locale, 'fromBot', { name: meta.fromBotName }) : t(locale, 'fromAnotherBot');
  if (meta.deviceName && meta.deviceName === thisDevice) return t(locale, 'fromThisDevice');
  return meta.deviceName ? t(locale, 'fromDevice', { name: meta.deviceName }) : t(locale, 'fromDevice', { name: '?' });
}

/** The conversation as rows: the reducer's items, grouped the way the desktop groups them. */
export function TranscriptView({ locale, transcript, running, deviceName, fetchImage }: TranscriptViewProps) {
  const rows = useMemo(
    () => {
      const items = itemsWithExtras(transcript.conversation.items, transcript.extras, (extra) => describeExtra(locale, extra));
      return placeTranscriptAgents(transcriptRows(items, running), items, transcript.agents ?? [],
        transcript.agentAnchors ?? anchorTranscriptAgents(new Map(), items, transcript.agents ?? []));
    },
    [transcript, running, locale],
  );
  return (
    <div className="rc-transcript">
      {rows.map((row) => (
        <Row key={row.id} locale={locale} row={row} transcript={transcript} deviceName={deviceName} fetchImage={fetchImage} />
      ))}
    </div>
  );
}

function Row({ locale, row, transcript, deviceName, fetchImage }: { locale: Locale; row: AgentTranscriptRow; transcript: Transcript; deviceName: string; fetchImage(ref: string): Promise<string> }) {
  switch (row.type) {
    case 'agents':
      return <AgentGroup locale={locale} agents={row.agents} launches={row.launches} />;
    case 'tool-group':
      return (
        <ToolGroup locale={locale} tools={row.tools} trimmed={transcript.truncatedTools} />
      );
    case 'narration':
      return <Narration locale={locale} item={row} meta={transcript.promptMeta.get(row.id)} deviceName={deviceName} fetchImage={fetchImage} />;
    case 'thinking':
      return <p className="rc-thinking" role="status">{t(locale, 'thinking')}</p>;
    case 'meta':
      return row.dur || row.files?.length ? (
        <p className="rc-meta">
          {displayText(row.dur, 120)}
          {row.files?.length ? ` · ${row.files.length} file${row.files.length === 1 ? '' : 's'}` : ''}
        </p>
      ) : null;
    case 'compaction':
      return <p className="rc-note">{displayText(row.detail ?? row.status, 120)}</p>;
    case 'command':
      return <pre className="rc-code rc-command">{row.output}</pre>;
    default:
      return null;
  }
}

function Narration({ locale, item, meta, deviceName, fetchImage }: { locale: Locale; item: Extract<RunItem, { type: 'narration' }>; meta: PromptMeta | undefined; deviceName: string; fetchImage(ref: string): Promise<string> }) {
  if (item.role === 'user') {
    return (
      <div className="rc-user">
        {meta ? <span className="rc-origin">{originLabel(locale, meta, deviceName)}</span> : null}
        <div className="rc-bubble">
          {item.text ? <p className="rc-plain">{item.text}</p> : null}
          <Images images={item.images ?? []} locale={locale} fetchImage={fetchImage} />
        </div>
      </div>
    );
  }
  if (item.role === 'assistant') {
    return (
      <div className="rc-assistant">
        <Markdown text={item.text} />
      </div>
    );
  }
  return <p className={`rc-note${item.tone === 'danger' ? ' rc-danger' : ''}`}>{item.text}</p>;
}

function Images({ images, locale, fetchImage }: { images: readonly { media_type: string; url: string }[]; locale: Locale; fetchImage(ref: string): Promise<string> }) {
  if (images.length === 0) return null;
  return (
    <div className="rc-images">
      {images.map((image, index) => (
        <RemoteImage key={`${index}:${image.url.slice(0, 64)}`} url={image.url} locale={locale} fetchImage={fetchImage} />
      ))}
    </div>
  );
}

/** A `data:` image shows at once; anything else is a reference to ask the desktop for, only when tapped. */
function RemoteImage({ url, locale, fetchImage }: { url: string; locale: Locale; fetchImage(ref: string): Promise<string> }) {
  const ref = imageRefOf(url);
  const [state, setState] = useState<{ status: 'idle' | 'loading' | 'failed' } | { status: 'ready'; src: string }>({ status: 'idle' });
  if (ref === undefined) return <img className="rc-image" src={url} alt={t(locale, 'imageAlt')} />;
  if (state.status === 'ready') return <img className="rc-image" src={state.src} alt={t(locale, 'imageAlt')} />;
  const load = (): void => {
    setState({ status: 'loading' });
    fetchImage(ref).then((src) => setState({ status: 'ready', src }), () => setState({ status: 'failed' }));
  };
  return (
    <button type="button" className="rc-image-ref" onClick={load} disabled={state.status === 'loading'}>
      {state.status === 'loading' ? t(locale, 'imageLoading') : state.status === 'failed' ? t(locale, 'imageFailed') : t(locale, 'imageLoad')}
    </button>
  );
}

function ToolGlyph({ tool }: { tool: ToolRunItem }) {
  const task = isTaskProgressTool(tool.tool);
  const send = /(?:^|__)(?:SendMessage|send_message)$/i.test(tool.tool);
  const kind = tool.tool === 'LocalApp' ? 'local-app' : task ? 'tasks' : send ? 'send' : tool.tool === 'ToolSearch' ? 'search' : tool.view.verb;
  let artwork;
  switch (kind) {
    case 'local-app': artwork = <><rect x="2" y="2" width="4.5" height="4.5" rx="1.2" /><rect x="9.5" y="2" width="4.5" height="4.5" rx="1.2" /><rect x="2" y="9.5" width="4.5" height="4.5" rx="1.2" /><rect x="9.5" y="9.5" width="4.5" height="4.5" rx="1.2" /></>; break;
    case 'tasks': artwork = <><path d="m1.8 3.3 1.1 1.1 2-2.2M7 3.5h7M7 8h7M7 12.5h7" /><circle cx="3.2" cy="8" r="1.1" /><circle cx="3.2" cy="12.5" r="1.1" /></>; break;
    case 'send': artwork = <path d="m14 2-4.5 12-2.6-5-5-2.5L14 2ZM7 9l7-7" />; break;
    case 'search': artwork = <><circle cx="6.7" cy="6.7" r="4.5" /><path d="m10 10 3.5 3.5" /></>; break;
    case 'shell': case 'output': artwork = <><rect x="1.5" y="2" width="13" height="12" rx="2" /><path d="m4 5 2.5 3L4 11m5 0h3" /></>; break;
    case 'fetch': artwork = <><circle cx="8" cy="8" r="6" /><ellipse cx="8" cy="8" rx="2.5" ry="6" /><path d="M2 8h12" /></>; break;
    case 'create': case 'update': artwork = <><path d="m3 10 7.5-7.5a1.5 1.5 0 0 1 2 2L5 12l-3 1 1-3Zm6-6 3 3M8 14h6" /></>; break;
    case 'read': artwork = <><path d="M8 3c-2-2-5-1-6 0v10c2-1 4-1 6 0 2-1 4-1 6 0V3c-1-1-4-2-6 0Zm0 0v10" /></>; break;
    case 'kill': artwork = <rect x="3" y="3" width="10" height="10" rx="2" />; break;
    case 'skill': artwork = <path d="m8 1 1.7 5.3L15 8l-5.3 1.7L8 15l-1.7-5.3L1 8l5.3-1.7L8 1Z" />; break;
    default: artwork = <path d="M10 2a4 4 0 0 0-5 5L1.5 11a2 2 0 0 0 3 3L9 10a4 4 0 0 0 5-5l-3 2-2-2 1-3Z" />;
  }
  return <svg className="rc-tool-glyph" data-tool-icon={kind} width="18" height="18" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.35" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{artwork}</svg>;
}

function AgentGroup({ locale, agents, launches }: { locale: Locale; agents: readonly SessionAgentSummaryDto[]; launches: ReadonlyMap<string, ToolRunItem> }) {
  const [open, setOpen] = useState(false);
  const status = (agent: SessionAgentSummaryDto): string => {
    if (['running', 'working'].includes(agent.status)) return t(locale, 'agentStarted');
    if (agent.report_delivery === 'withheld') return t(locale, 'agentWithheld');
    if (agent.report_delivery === 'pending') return t(locale, 'agentPending');
    if (agent.report_delivery === 'flagged') return t(locale, 'agentFlagged');
    if (agent.status === 'completed') return t(locale, agent.report_delivery === 'delivered' ? 'agentFinished' : 'agentExecutionEnded');
    if (['failed', 'error'].includes(agent.status)) return t(locale, 'toolFailed');
    return agent.status;
  };
  const attention = (agent: SessionAgentSummaryDto) => ['failed', 'error'].includes(agent.status) || agent.report_delivery === 'withheld';
  const states = new Map<string, { count: number; failed: boolean }>();
  for (const agent of agents) { const label = status(agent); const old = states.get(label); states.set(label, { count: (old?.count ?? 0) + 1, failed: attention(agent) }); }
  const name = (agent: SessionAgentSummaryDto) => agent.name.trim() || agent.agent_type || 'Agent';
  const avatar = (agent: SessionAgentSummaryDto) => <picture className="rc-agent-avatar" key={agent.agent_id} aria-hidden="true">
    <img className="rc-avatar-dark" src={AGENT_AVATARS[agentAvatarIndex(agent.agent_id)].dark} width="18" height="18" alt="" />
    <img className="rc-avatar-light" src={AGENT_AVATARS[agentAvatarIndex(agent.agent_id)].light} width="18" height="18" alt="" />
  </picture>;
  const member = (agent: SessionAgentSummaryDto) => <details className="rc-agent-member" key={agent.agent_id}>
    <summary>{avatar(agent)}<span className="rc-inline-label">{name(agent)}</span><span className={attention(agent) ? 'rc-danger' : 'rc-inline-state'}>{status(agent)}</span><span className="rc-inline-arrow" aria-hidden="true">›</span></summary>
    <p className="rc-note">{[agent.agent_type, launches.get(agent.agent_id)?.view.primary, agent.latest_activity !== 'SubagentHandback' && agent.latest_activity].filter(Boolean).join(' · ') || status(agent)}</p>
    {launches.get(agent.agent_id)?.result?.body ? <pre className="rc-code">{launches.get(agent.agent_id)!.result!.body}</pre> : null}
  </details>;
  if (agents.length === 1) return <div className="rc-agent-group">{member(agents[0]!)}</div>;
  return <div className="rc-agent-group">
    <button className="rc-inline-row" type="button" aria-expanded={open} onClick={() => setOpen(!open)} title={agents.map(agent => `${name(agent)} · ${status(agent)}`).join('; ')}>
      <span className="rc-agent-avatars">{agents.slice(0, 3).map(avatar)}{agents.length > 3 ? <span>+{agents.length - 3}</span> : null}</span>
      <span className="rc-inline-label">{agents.map(name).join(locale === 'zh' ? '、' : ', ')}</span>
      <span className="rc-inline-state">{[...states].sort((a, b) => Number(b[1].failed) - Number(a[1].failed)).map(([label, state], i) => <span key={label} className={state.failed ? 'rc-danger' : undefined}>{i ? ' · ' : ''}{states.size > 1 ? `${state.count} ` : ''}{label}</span>)}</span>
      <span className="rc-inline-arrow" aria-hidden="true">{open ? '⌄' : '›'}</span>
    </button>
    {open ? <div className="rc-inline-members">{agents.map(member)}</div> : null}
  </div>;
}

function ToolGroup({ locale, tools, trimmed }: { locale: Locale; tools: ToolRunItem[]; trimmed: ReadonlySet<string> }) {
  const [open, setOpen] = useState(false);
  if (tools.length === 1) return <ToolCard locale={locale} tool={tools[0]!} trimmed={trimmed.has(tools[0]!.id)} />;
  const workflow = tools.every(isTaskWorkflowTool);
  const first = tools[0]!;
  const running = tools.filter(tool => tool.status === 'running').length;
  const failed = tools.filter(tool => tool.status === 'error').length;
  const label = workflow ? t(locale, tools.every(tool => isTaskProgressTool(tool.tool)) ? 'taskProgress' : 'taskCoordination') : groupLabel(locale, first);
  return <div className="rc-tools">
    <button className="rc-inline-row" type="button" aria-expanded={open} onClick={() => setOpen(!open)}>
      <ToolGlyph tool={workflow ? { ...first, tool: 'TaskList' } : first} />
      <span className="rc-inline-label">{label} · {t(locale, 'toolCalls', { count: tools.length })}</span>
      {running ? <span className="rc-inline-state">{t(locale, 'toolRunningCount', { count: running })}</span> : null}
      {workflow && failed ? <span className="rc-danger">{t(locale, 'toolFailedCount', { count: failed })}</span> : null}
      <span className="rc-inline-arrow" aria-hidden="true">{open ? '⌄' : '›'}</span>
    </button>
    {open || (!workflow && running > 0) ? <div className="rc-inline-members">{tools.filter(tool => open || tool.status === 'running').map(tool => <ToolCard key={tool.id} locale={locale} tool={tool} trimmed={trimmed.has(tool.id)} />)}</div> : null}
  </div>;
}

function groupLabel(locale: Locale, tool: ToolRunItem): string {
  switch (tool.view.verb) {
    case 'read': return t(locale, 'groupRead');
    case 'search': return t(locale, 'groupSearch');
    case 'shell': return t(locale, 'groupShell');
    case 'fetch': return t(locale, 'groupFetch');
    default: return toolDisplayHeader(tool).label || tool.tool;
  }
}

function toolLabel(locale: Locale, tool: ToolRunItem): string {
  if (tool.tool === 'LocalApp') return [t(locale, 'localApp'), tool.view.primary || (tool.status === 'error' ? undefined : tool.result?.headline)].filter(Boolean).join(' ');
  const argument = tool.view.primary || (tool.view.verb === 'shell' ? tool.view.sub_line?.text : undefined);
  if (argument && ['read', 'search', 'shell', 'fetch', 'create', 'update'].includes(tool.view.verb)) {
    return `${groupLabel(locale, tool)} ${argument}`;
  }
  return tool.view.primary ? `${tool.view.label || tool.tool} ${tool.view.primary}${tool.view.qualifier ? ` ${tool.view.qualifier}` : ''}` : toolDisplayHeader(tool).title || tool.tool;
}

function ToolCard({ locale, tool, trimmed }: { locale: Locale; tool: ToolRunItem; trimmed: boolean }) {
  const [open, setOpen] = useState(false);
  const body = toolHasBody(tool);
  const notice = toolTruncationNotice(tool);
  return (
    <div className={`rc-tool rc-tool-${tool.status}`}>
      <button type="button" className="rc-tool-head" onClick={() => setOpen((value) => !value)} disabled={!body} aria-expanded={body ? open : undefined}>
        <ToolGlyph tool={tool} /><span className="rc-tool-title" title={displayText(toolLabel(locale, tool), 1000)}>{displayText(toolLabel(locale, tool), 300)}</span>
        {tool.status === 'running' ? <span className="rc-tool-state">{t(locale, 'toolRunning')}</span> : null}
        {tool.status === 'error' ? <span className="rc-tool-state rc-danger">{t(locale, 'toolFailed')}</span> : null}
        {body ? <span className="rc-inline-arrow" aria-hidden="true">{open ? '⌄' : '›'}</span> : null}
      </button>
      {open && tool.result?.headline ? <p className="rc-tool-headline">{displayText(tool.result.headline, 300)}</p> : null}
      {open && tool.result?.diff ? <Diff diff={tool.result.diff} /> : null}
      {open && tool.result?.body ? <pre className="rc-code">{tool.result.body}</pre> : null}
      {open && notice ? <p className="rc-note">{notice}</p> : null}
      {trimmed ? <p className="rc-note">{t(locale, 'truncatedTool')}</p> : null}
    </div>
  );
}

function Diff({ diff }: { diff: StructuredDiffDto }) {
  return (
    <pre className="rc-code rc-diff">
      {diff.file_path ? <span className="rc-diff-file">{displayText(diff.file_path, 300)}{'\n'}</span> : null}
      {diff.rows.map((row, index) => (
        <span key={index} className={`rc-diff-${row.kind}`}>
          {row.kind === 'add' ? '+' : row.kind === 'remove' ? '-' : ' '}
          {row.segments.map((segment, i) => (
            <span key={i} className={segment.emph ? 'rc-diff-emph' : undefined}>{segment.text}</span>
          ))}
          {'\n'}
        </span>
      ))}
    </pre>
  );
}
