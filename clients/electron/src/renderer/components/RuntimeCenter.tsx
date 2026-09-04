import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from 'react';
import type { PlanTaskDto, SessionAgentSummaryDto, TaskRowDto } from '@lingxi/bridge-client';

import { conversationFromMessages } from '../bridge/conversation';
import { orderedTasks } from '../bridge/desktopState';
import type { UseBridge } from '../bridge/useBridge';
import {
  planRuntimeItemId,
  runtimeCenterItemKey,
  sameRuntimeCenterItem,
  type RuntimeCenterItemRef,
  type RuntimeCenterSection,
  type RuntimeResource,
} from '../bridge/runtimeCenterState';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';
import { Stage } from './Stage';

const SECTION_LABELS: Record<RuntimeCenterSection, string> = {
  tasks: 'Tasks',
  agents: 'Agents',
  resources: 'Resources',
  plan: 'Plan',
};

const statusRank: Record<string, number> = {
  running: 0,
  pending: 1,
  idle: 2,
  failed: 3,
  killed: 4,
  cancelled: 5,
  completed: 6,
};

const IN_FLIGHT_TASK_STATUSES: ReadonlySet<string> = new Set(['pending', 'running', 'paused']);

/**
 * [Finding 5] True when any listed task is still in flight. The desktop
 * client's `TaskRow` (and `task.stage`/`status`/`description`, read by
 * `TaskDetail` and the overview subtitle) is pull-only -- only as fresh as
 * the last `task_list` refresh -- so this drives whether a caller needs to
 * keep re-pulling it. Exported so the wiring below (into
 * `bridge.refreshTasks()`) can be unit-tested without mounting React.
 */
export function hasInFlightTask(tasks: readonly { status: { type: string } }[]): boolean {
  return tasks.some((task) => IN_FLIGHT_TASK_STATUSES.has(task.status.type));
}

/**
 * [Finding 5] Starts (and returns a stopper for) the interval that keeps a
 * pull-only desktop resource fresh while it is in flight. Before this
 * existed, nothing re-delivered `TaskRow` while a task ran -- a running
 * `/fusion` task's stage line (and the overview subtitle beside it) froze
 * at whatever it read the instant the pane opened, for the task's entire
 * remaining lifetime, no matter how many `set_fusion_stage` updates the
 * engine emitted in the meantime.
 *
 * [Finding 5, rework round 2] `isActive` is a GETTER, read fresh on every
 * tick, not a snapshot captured when the interval was created. The earlier
 * shape took a plain `boolean` and was wired straight from a `useEffect`
 * dependency (`[..., hasActiveTask, ...]`); because `refresh()` cleared the
 * task map before repopulating it (see `requestTaskList`'s now-`preserve`d
 * wipe), `hasActiveTask` flipped false for one render on every single
 * poll, which re-ran the effect and issued another poll -- a
 * self-amplifying `task_list` storm with no interval ticks required at
 * all. Reading `isActive()` per tick, from a ref the caller updates every
 * render, means the interval itself never needs to be torn down and
 * restarted just because the in-flight flag flickered, so nothing about
 * this refresh can ever retrigger the effect that started it. `refresh` is
 * always called with `{ preserve: true }`: a background poll must merge
 * fresh rows over the existing map, never blank it first.
 */
export function startPollingWhileActive(
  isActive: () => boolean,
  refresh: (options?: { preserve?: boolean }) => Promise<void>,
  intervalMs = 1_500,
): () => void {
  const timer = setInterval(() => {
    if (!isActive()) return;
    void refresh({ preserve: true }).catch(() => undefined);
  }, intervalMs);
  return () => clearInterval(timer);
}

function statusColor(t: ReturnType<typeof useT>, status: string): string {
  if (status === 'completed') return t.ok;
  if (status === 'failed' || status === 'killed' || status === 'cancelled') return t.danger;
  if (status === 'running' || status === 'in_progress') return t.accent;
  return t.text4;
}

function shorten(value: string, max = 90): string {
  const trimmed = value.trim();
  return trimmed.length > max ? `${trimmed.slice(0, max - 1)}…` : trimmed;
}

function EmptyRow({ children }: { children: ReactNode }) {
  const t = useT();
  return <div style={{ padding: '7px 9px 9px', color: t.text4, fontSize: 11 }}>{children}</div>;
}

function OverviewRow({
  icon,
  title,
  subtitle,
  status,
  onClick,
}: {
  icon: string;
  title: string;
  subtitle?: string;
  status?: string;
  onClick(): void;
}) {
  const t = useT();
  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        width: '100%', display: 'flex', alignItems: 'center', gap: 9, minHeight: 42,
        padding: '7px 9px', border: 0, borderRadius: 8, background: 'transparent',
        color: t.text, textAlign: 'left', cursor: 'pointer', font: 'inherit',
      }}
      onMouseEnter={(event) => { event.currentTarget.style.background = t.surfaceHover; }}
      onMouseLeave={(event) => { event.currentTarget.style.background = 'transparent'; }}
    >
      <span style={{ width: 23, height: 23, display: 'grid', placeItems: 'center', flexShrink: 0, borderRadius: 7, background: t.surfaceActive, color: status ? statusColor(t, status) : t.accent }}>
        <Icon name={icon} size={13} stroke={1.8} />
      </span>
      <span style={{ minWidth: 0, flex: 1 }}>
        <span style={{ display: 'block', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 12, fontWeight: 560 }}>{title}</span>
        {subtitle && <span style={{ display: 'block', marginTop: 2, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: t.text4, fontSize: 10.5 }}>{subtitle}</span>}
      </span>
      {status && <span className={status === 'running' ? 'running-sweep' : undefined} style={{ flexShrink: 0, color: statusColor(t, status), fontSize: 10 }}>{status}</span>}
    </button>
  );
}

function OverviewSection({
  section,
  open,
  onToggle,
  count,
  children,
}: {
  section: RuntimeCenterSection;
  open: boolean;
  onToggle(): void;
  count: number;
  children: ReactNode;
}) {
  const t = useT();
  return (
    <section style={{ borderBottom: `0.5px solid ${t.border}`, paddingBottom: 5, marginBottom: 5 }}>
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        style={{ width: '100%', minHeight: 31, display: 'flex', alignItems: 'center', gap: 7, padding: '5px 8px', border: 0, borderRadius: 7, background: 'transparent', color: t.text2, font: 'inherit', cursor: 'pointer', textAlign: 'left' }}
      >
        <Icon name={open ? 'chevron' : 'chevronR'} size={12} stroke={1.9} />
        <span style={{ flex: 1, fontSize: 10.5, fontWeight: 680, letterSpacing: 0.8, textTransform: 'uppercase' }}>{SECTION_LABELS[section]}</span>
        <span className="mono" style={{ color: t.text4, fontSize: 10 }}>{count}</span>
      </button>
      {open && <div>{children}</div>}
    </section>
  );
}

export function RuntimeCenterOverview({ bridge }: { bridge: UseBridge }) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const center = bridge.runtimeCenter;
  const tasks = orderedTasks(bridge.desktop);
  const agents = useMemo(() => Object.values(center.agents)
    .filter((agent) => agent.agent_id !== 'main')
    .sort((left, right) => (statusRank[left.status] ?? 99) - (statusRank[right.status] ?? 99) || (right.updated_at_ms ?? 0) - (left.updated_at_ms ?? 0)), [center.agents]);
  const plan = center.plan.length > 0 ? center.plan : bridge.conversation.plan;
  const openerRef = useRef<HTMLElement | null>(null);
  const hasActiveTask = hasInFlightTask(tasks);
  // [Finding 5, rework round 2] Read every poll tick, never a `useEffect`
  // dependency -- see `startPollingWhileActive`'s doc comment for why
  // putting `hasActiveTask` in the effect's own dependency array storms.
  const hasActiveTaskRef = useRef(hasActiveTask);
  hasActiveTaskRef.current = hasActiveTask;

  const closeOverview = useCallback((restoreFocus: boolean) => {
    bridge.setRuntimeCenterOverviewOpen(false);
    if (!restoreFocus) return;
    window.requestAnimationFrame(() => {
      const fallback = document.querySelector<HTMLElement>('[data-runtime-center-trigger="true"]');
      (openerRef.current?.isConnected ? openerRef.current : fallback)?.focus();
    });
  }, [bridge.setRuntimeCenterOverviewOpen]);

  useEffect(() => {
    if (!center.overviewOpen) return undefined;
    openerRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : document.querySelector<HTMLElement>('[data-runtime-center-trigger="true"]');
    const focusFrame = window.requestAnimationFrame(() => {
      ref.current?.querySelector<HTMLElement>('[data-runtime-overview-initial="true"]')?.focus();
    });
    const onPointerDown = (event: PointerEvent) => {
      if (event.target instanceof Node && !ref.current?.contains(event.target)) closeOverview(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        closeOverview(true);
        return;
      }
      if (event.key === 'Tab' && ref.current) {
        const focusable = [...ref.current.querySelectorAll<HTMLElement>('button:not([disabled])')];
        if (focusable.length === 0) return;
        const active = document.activeElement;
        const index = active instanceof HTMLElement ? focusable.indexOf(active) : -1;
        const target = index < 0
          ? (event.shiftKey ? focusable.at(-1) : focusable[0])
          : event.shiftKey && index === 0
            ? focusable.at(-1)
            : !event.shiftKey && index === focusable.length - 1
              ? focusable[0]
              : undefined;
        if (target) {
          event.preventDefault();
          target.focus();
        }
      }
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      window.cancelAnimationFrame(focusFrame);
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [center.overviewOpen, closeOverview]);

  // [Finding 5] The desktop client has no server-pushed `TaskRow` update --
  // `bridge.desktop.tasks` (and therefore the `task.stage`/`status`
  // subtitle rendered above) is only as fresh as the last `task_list`
  // pull. Some entry points (the toolbar `+` button, `onClick={() =>
  // bridge.setRuntimeCenterOverviewOpen(true)}`) open this panel without
  // issuing that pull at all, so without this it can render whatever
  // stale row an unrelated earlier refresh happened to leave behind, then
  // stay frozen on it for the rest of a multi-minute run. Fetch once on
  // open, then keep polling while any listed task is still
  // pending/running/paused.
  //
  // [Finding 5, rework round 2] `hasActiveTask` is deliberately NOT a
  // dependency here -- see `hasActiveTaskRef` and
  // `startPollingWhileActive` above. The effect is keyed only on whether
  // the overview is open, so it mounts exactly one interval per open and
  // never tears it down and restarts mid-poll.
  useEffect(() => {
    if (!center.overviewOpen) return undefined;
    void bridge.refreshTasks().catch(() => undefined);
    return startPollingWhileActive(() => hasActiveTaskRef.current, bridge.refreshTasks);
  }, [center.overviewOpen, bridge.refreshTasks]);

  if (!center.overviewOpen) return null;
  const open = (item: RuntimeCenterItemRef) => {
    const key = runtimeCenterItemKey(item);
    bridge.openRuntimeItem(item);
    window.requestAnimationFrame(() => {
      const tabs = document.querySelectorAll<HTMLElement>('[data-runtime-inspector-tab]');
      [...tabs].find((tab) => tab.dataset.runtimeInspectorTab === key)?.focus();
    });
  };
  const section = (name: RuntimeCenterSection) => center.sections[name];
  return (
    <div
      ref={ref}
      id="runtime-center-overview"
      role="dialog"
      aria-label="Runtime center"
      style={{
        position: 'absolute', top: 59, right: 14, zIndex: 40, width: 340, maxHeight: 'min(78vh, 680px)', overflowY: 'auto',
        padding: '9px 8px 5px', border: `0.5px solid ${t.borderStrong}`, borderRadius: 15,
        background: t.surface, boxShadow: '0 20px 55px rgba(0,0,0,.28)', animation: 'fade-in .14s ease',
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '4px 9px 9px' }}>
        <span style={{ flex: 1, color: t.text, fontSize: 13, fontWeight: 700 }}>Runtime center</span>
        <span className="mono" style={{ color: t.text4, fontSize: 10 }}>{tasks.length + agents.length + center.resources.length + plan.length} items</span>
        <button type="button" data-runtime-overview-initial="true" aria-label="Close runtime center" onClick={() => closeOverview(true)} style={{ display: 'grid', placeItems: 'center', width: 24, height: 24, border: 0, borderRadius: 6, background: 'transparent', color: t.text3, cursor: 'pointer' }}><Icon name="x" size={13} /></button>
      </div>
      <OverviewSection section="tasks" open={section('tasks')} onToggle={() => bridge.toggleRuntimeCenterSection('tasks')} count={tasks.length}>
        {tasks.length === 0 ? <EmptyRow>No background tasks.</EmptyRow> : tasks.map((task) => (
          <OverviewRow key={task.task_id} icon="activity" title={task.task_type} subtitle={shorten(task.status.type === 'running' && task.stage ? task.stage : task.description)} status={task.status.type} onClick={() => open({ kind: 'task', id: task.task_id })} />
        ))}
      </OverviewSection>
      <OverviewSection section="agents" open={section('agents')} onToggle={() => bridge.toggleRuntimeCenterSection('agents')} count={agents.length}>
        {agents.length === 0 ? <EmptyRow>No subagents reported yet.</EmptyRow> : agents.map((agent) => (
          <OverviewRow key={agent.agent_id} icon="sparkle" title={agent.name || agent.agent_type} subtitle={shorten(agent.latest_activity ?? agent.agent_type)} status={agent.status} onClick={() => open({ kind: 'agent', id: agent.agent_id })} />
        ))}
      </OverviewSection>
      <OverviewSection section="resources" open={section('resources')} onToggle={() => bridge.toggleRuntimeCenterSection('resources')} count={center.resources.length}>
        {center.resources.length === 0 ? <EmptyRow>No input resources in this session.</EmptyRow> : center.resources.map((resource) => (
          <OverviewRow key={resource.id} icon={resource.kind === 'image' ? 'image' : resource.kind === 'file' ? 'file' : 'anchor'} title={resource.name} subtitle={resource.path ?? resource.detail} onClick={() => open({ kind: 'resource', id: resource.id })} />
        ))}
      </OverviewSection>
      <OverviewSection section="plan" open={section('plan')} onToggle={() => bridge.toggleRuntimeCenterSection('plan')} count={plan.length}>
        {plan.length === 0 ? <EmptyRow>No active plan.</EmptyRow> : plan.map((task, index) => (
          <OverviewRow key={planRuntimeItemId(task, index, plan)} icon={task.state === 'completed' ? 'check' : 'goal'} title={task.subject} status={task.state} onClick={() => open({ kind: 'plan', id: planRuntimeItemId(task, index, plan) })} />
        ))}
      </OverviewSection>
    </div>
  );
}

function tabLabel(item: RuntimeCenterItemRef, bridge: UseBridge): string {
  if (item.kind === 'task') return bridge.desktop.tasks[item.id]?.task_type ?? 'Task';
  if (item.kind === 'agent') return bridge.runtimeCenter.agents[item.id]?.name ?? item.id.slice(0, 13);
  if (item.kind === 'resource') return bridge.runtimeCenter.resources.find((resource) => resource.id === item.id)?.name ?? 'Resource';
  const plan = bridge.runtimeCenter.plan.length > 0 ? bridge.runtimeCenter.plan : bridge.conversation.plan;
  const index = plan.findIndex((task, i) => planRuntimeItemId(task, i, plan) === item.id);
  return index >= 0 ? plan[index]?.subject ?? 'Plan item' : 'Plan';
}

function InspectorHeader({ title, subtitle, onClose }: { title: string; subtitle?: string; onClose(): void }) {
  const t = useT();
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 8, minHeight: 50, padding: '0 12px', borderBottom: `0.5px solid ${t.border}` }}>
      <span style={{ minWidth: 0, flex: 1 }}>
        <span style={{ display: 'block', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: t.text, fontSize: 12.5, fontWeight: 680 }}>{title}</span>
        {subtitle && <span style={{ display: 'block', marginTop: 2, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: t.text4, fontSize: 10 }}>{subtitle}</span>}
      </span>
      <button type="button" aria-label="Collapse runtime inspector" onClick={onClose} style={{ display: 'grid', placeItems: 'center', width: 26, height: 26, border: 0, borderRadius: 6, background: 'transparent', color: t.text3, cursor: 'pointer' }}><Icon name="sidebarR" size={14} /></button>
    </div>
  );
}

function InspectorTabs({ bridge }: { bridge: UseBridge }) {
  const t = useT();
  const center = bridge.runtimeCenter;
  const focusTab = (item: RuntimeCenterItemRef) => {
    const key = runtimeCenterItemKey(item);
    const tabs = document.querySelectorAll<HTMLElement>('[data-runtime-inspector-tab]');
    [...tabs].find((tab) => tab.dataset.runtimeInspectorTab === key)?.focus();
  };
  const selectFromKeyboard = (
    event: ReactKeyboardEvent<HTMLButtonElement>,
    index: number,
  ) => {
    let target: RuntimeCenterItemRef | undefined;
    if (event.key === 'ArrowLeft') target = center.tabs[(index - 1 + center.tabs.length) % center.tabs.length];
    if (event.key === 'ArrowRight') target = center.tabs[(index + 1) % center.tabs.length];
    if (event.key === 'Home') target = center.tabs[0];
    if (event.key === 'End') target = center.tabs.at(-1);
    if (!target) return;
    event.preventDefault();
    bridge.openRuntimeItem(target);
    focusTab(target);
  };
  return (
    <div role="tablist" aria-label="Runtime inspector items" style={{ display: 'flex', gap: 5, overflowX: 'auto', padding: '8px 9px 7px', borderBottom: `0.5px solid ${t.border}`, scrollbarWidth: 'thin' }}>
      {center.tabs.map((item, index) => {
        const active = sameRuntimeCenterItem(item, center.activeItem);
        const key = runtimeCenterItemKey(item);
        return (
          <div key={`${item.kind}:${item.id}`} role="presentation" style={{ display: 'flex', alignItems: 'center', flexShrink: 0, borderRadius: 7, background: active ? t.accentBg : t.surfaceActive, color: active ? t.accent : t.text3 }}>
            <button type="button" role="tab" id={`runtime-inspector-tab-${encodeURIComponent(key)}`} aria-controls="runtime-inspector-panel" aria-selected={active} tabIndex={active ? 0 : -1} data-runtime-inspector-tab={key} data-runtime-inspector-active={active ? 'true' : undefined} onClick={() => bridge.openRuntimeItem(item)} onKeyDown={(event) => selectFromKeyboard(event, index)} style={{ maxWidth: 150, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', padding: '5px 7px 5px 9px', border: 0, background: 'transparent', color: 'inherit', font: 'inherit', fontSize: 10.5, cursor: 'pointer' }}>{tabLabel(item, bridge)}</button>
            <button type="button" aria-label={`Close ${tabLabel(item, bridge)}`} onClick={() => { bridge.closeRuntimeItem(item); window.requestAnimationFrame(() => (document.querySelector<HTMLElement>('[data-runtime-inspector-active="true"]') ?? document.querySelector<HTMLElement>('[data-runtime-center-trigger="true"]'))?.focus()); }} style={{ display: 'grid', placeItems: 'center', width: 22, height: 22, marginRight: 2, border: 0, borderRadius: 5, background: 'transparent', color: 'inherit', cursor: 'pointer' }}><Icon name="x" size={11} /></button>
          </div>
        );
      })}
      <button type="button" aria-label="Open runtime center" onClick={() => bridge.setRuntimeCenterOverviewOpen(true)} style={{ display: 'grid', placeItems: 'center', flexShrink: 0, width: 26, height: 26, border: `0.5px solid ${t.border}`, borderRadius: 7, background: 'transparent', color: t.text3, cursor: 'pointer' }}><Icon name="plus" size={13} /></button>
    </div>
  );
}

export function TaskDetail({ task, bridge }: { task: TaskRowDto | undefined; bridge: UseBridge }) {
  const t = useT();
  const output = task ? bridge.desktop.taskOutput[task.task_id] : undefined;
  if (!task) return <EmptyRow>Task is no longer available.</EmptyRow>;
  return (
    <div style={{ padding: 15, overflowY: 'auto' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, color: statusColor(t, task.status.type), fontSize: 11 }}><Icon name="activity" size={14} /><span>{task.status.type}</span></div>
      {task.stage && <div style={{ marginTop: 6, color: t.text3, fontSize: 11 }}>{task.stage}</div>}
      <p style={{ margin: '12px 0', color: t.text, fontSize: 13, lineHeight: 1.55 }}>{task.description}</p>
      {output ? <pre className="mono" style={{ margin: 0, padding: 11, borderRadius: 9, background: t.windowBg, border: `0.5px solid ${t.border}`, color: t.text2, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', fontSize: 10.5, lineHeight: 1.55 }}>{output.content || '(No output yet)'}{output.truncated ? `\n\n…${output.totalLines} lines total` : ''}</pre> : <EmptyRow>Loading task output…</EmptyRow>}
    </div>
  );
}

function AgentDetail({ agent, bridge }: { agent: SessionAgentSummaryDto | undefined; bridge: UseBridge }) {
  const t = useT();
  const center = bridge.runtimeCenter;
  if (!agent) return <EmptyRow>Agent is no longer available.</EmptyRow>;
  const transcript = center.transcripts[agent.agent_id];
  const conversation = conversationFromMessages(transcript?.messages ?? []);
  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      <div style={{ padding: '10px 13px', borderBottom: `0.5px solid ${t.border}`, color: t.text3, fontSize: 10.5 }}>
        <span style={{ color: statusColor(t, agent.status), fontWeight: 650 }}>{agent.status}</span>
        {agent.model && <span> · {agent.model}</span>}
        {agent.latest_activity && <span style={{ display: 'block', marginTop: 4, color: t.text4 }}>{shorten(agent.latest_activity, 150)}</span>}
      </div>
      <Stage liveItems={conversation.items} running={agent.status === 'running'} sessionKey={`agent:${agent.agent_id}`} emptyMessage="Waiting for the agent to emit its first message." />
    </div>
  );
}

function ResourceDetail({ resource, bridge }: { resource: RuntimeResource | undefined; bridge: UseBridge }) {
  const t = useT();
  const [preview, setPreview] = useState<Awaited<ReturnType<UseBridge['previewWorkspaceFile']>> | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setPreview(null);
    setError(null);
    if (!resource?.path) return undefined;
    let cancelled = false;
    void bridge.previewWorkspaceFile(resource.path).then((value) => {
      if (!cancelled) setPreview(value);
    }).catch((cause: unknown) => {
      if (!cancelled) setError(cause instanceof Error ? cause.message : 'Unable to preview file.');
    });
    return () => { cancelled = true; };
  }, [bridge.previewWorkspaceFile, resource?.id, resource?.path]);
  if (!resource) return <EmptyRow>Resource is no longer available.</EmptyRow>;
  if (resource.kind === 'image' && resource.url) {
    return <div style={{ padding: 15 }}><img src={resource.url} alt={resource.name} style={{ display: 'block', maxWidth: '100%', maxHeight: 500, margin: '0 auto', objectFit: 'contain', borderRadius: 10, background: t.windowBg }} /><div style={{ marginTop: 10, color: t.text3, fontSize: 11 }}>{resource.name}</div></div>;
  }
  if (resource.kind === 'file') {
    if (error) return <div role="alert" style={{ padding: 15, color: t.danger, fontSize: 11 }}>{error}</div>;
    if (!preview) return <EmptyRow>Loading file preview…</EmptyRow>;
    if (preview.kind === 'binary') return <div style={{ padding: 15, color: t.text3, fontSize: 11 }}>Binary attachment · {(preview.size / 1024).toFixed(1)} KiB</div>;
    return <div style={{ minHeight: 0, flex: 1, overflow: 'auto', padding: 15 }}><pre className="mono" style={{ margin: 0, color: t.text2, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', fontSize: 10.5, lineHeight: 1.6 }}>{preview.content}{preview.truncated ? '\n\n…preview truncated at 512 KiB' : ''}</pre></div>;
  }
  return <div style={{ padding: 15, color: t.text3, fontSize: 11 }}>{resource.detail ?? 'Engine attachment metadata.'}</div>;
}

function PlanDetail({ selected, plan }: { selected: PlanTaskDto | undefined; plan: readonly PlanTaskDto[] }) {
  const t = useT();
  return (
    <div style={{ minHeight: 0, overflow: 'auto', padding: 15 }}>
      {selected && <div style={{ marginBottom: 16, padding: 12, borderRadius: 10, background: t.accentBg, color: t.text, fontSize: 13, lineHeight: 1.5 }}><div style={{ color: t.accent, fontSize: 10, fontWeight: 700, letterSpacing: .7, textTransform: 'uppercase', marginBottom: 5 }}>{selected.state.replace('_', ' ')}</div>{selected.subject}</div>}
      <div style={{ color: t.text4, fontSize: 10, fontWeight: 700, letterSpacing: .7, textTransform: 'uppercase', marginBottom: 7 }}>Full plan</div>
      {plan.length === 0 ? <EmptyRow>No active plan.</EmptyRow> : plan.map((task, index) => <div key={planRuntimeItemId(task, index, plan)} style={{ display: 'flex', gap: 8, padding: '6px 0', color: task.state === 'completed' ? t.text4 : t.text2, fontSize: 12, textDecoration: task.state === 'completed' ? 'line-through' : undefined }}><span style={{ color: task.state === 'completed' ? t.ok : task.state === 'in_progress' ? t.accent : t.text4 }}>●</span><span>{task.subject}</span></div>)}
    </div>
  );
}

export function RuntimeCenterInspector({ bridge }: { bridge: UseBridge }) {
  const t = useT();
  const center = bridge.runtimeCenter;
  const active = center.activeItem;
  const plan = center.plan.length > 0 ? center.plan : bridge.conversation.plan;
  const resource = active?.kind === 'resource' ? center.resources.find((entry) => entry.id === active.id) : undefined;
  const task = active?.kind === 'task' ? bridge.desktop.tasks[active.id] : undefined;
  const agent = active?.kind === 'agent' ? center.agents[active.id] : undefined;
  const selectedPlan = active?.kind === 'plan' ? plan.find((taskEntry, index) => planRuntimeItemId(taskEntry, index, plan) === active.id) : undefined;
  // [Finding 5, rework round 2] Same ref treatment as the overview panel's
  // `hasActiveTaskRef` -- `task?.status.type` must not sit in the
  // row-refresh effect's dependency array below, or a background poll
  // that ever wiped the map (the pre-`preserve` shape) flips this every
  // cycle and storms. Read fresh on every poll tick instead.
  const taskInFlightRef = useRef(false);
  taskInFlightRef.current = !!task && hasInFlightTask([task]);

  useEffect(() => {
    if (!active || active.kind !== 'agent' || agent?.status !== 'running') return undefined;
    const refresh = () => { void bridge.loadSessionAgentTranscript(active.id).catch(() => undefined); };
    refresh();
    const timer = setInterval(refresh, 1_500);
    return () => clearInterval(timer);
  }, [active?.kind, active?.id, agent?.status, bridge.loadSessionAgentTranscript]);

  useEffect(() => {
    if (!active || active.kind !== 'task' || !task) return undefined;
    const refresh = () => { void bridge.taskOutput(active.id).catch(() => undefined); };
    refresh();
    if (!hasInFlightTask([task])) return undefined;
    const timer = setInterval(refresh, 1_500);
    return () => clearInterval(timer);
  }, [active?.kind, active?.id, bridge.taskOutput, task?.status.type]);

  // [Finding 5] `task.stage`/`task.status`/`task.description` (rendered by
  // `TaskDetail` and the overview subtitle) only ever change when a fresh
  // `TaskRow` arrives, and the desktop client has no server-pushed row
  // update while a task runs -- `bridge.desktop.tasks[id]` is only as
  // fresh as the last `task_list` pull. Without this, opening a running
  // task's detail pane freezes its stage line at whatever it read on open
  // for the task's entire remaining lifetime, even though the output pane
  // right next to it (the effect above) keeps ticking.
  //
  // [Finding 5, rework round 2] `task?.status.type` is deliberately NOT a
  // dependency here (see `taskInFlightRef` above) -- the effect mounts
  // once per selected task and the interval itself decides per tick
  // whether to poll, so a background refresh flipping the status can never
  // retrigger the effect that issued it.
  //
  // [Finding 22] `!!task` IS a dependency, unlike `task?.status.type`
  // above: the effect body early-returns on `!task`, so if the row is
  // still absent from `bridge.desktop.tasks` the moment this effect last
  // ran (e.g. a task tab selected during the one round-trip window a
  // non-preserve `task_list` refresh empties that map), no interval is
  // ever started, and nothing re-runs this effect when the row lands --
  // `task.stage` then freezes for the rest of the run while the output
  // poll effect above keeps ticking. `!!task` only flips on that one
  // arrival (or a genuine non-preserve wipe), so it cannot reintroduce the
  // per-tick storm `task?.status.type` was excluded to prevent.
  useEffect(() => {
    if (!active || active.kind !== 'task' || !task) return undefined;
    return startPollingWhileActive(() => taskInFlightRef.current, bridge.refreshTasks);
  }, [active?.kind, active?.id, !!task, bridge.refreshTasks]);

  if (!center.inspectorOpen || !active) return null;
  const subtitle = active.kind === 'agent' ? agent?.agent_type : active.kind === 'task' ? task?.status.type : active.kind === 'resource' ? resource?.path : undefined;
  return (
    <aside aria-label="Runtime inspector" style={{ width: 390, minWidth: 300, maxWidth: '42vw', flexShrink: 0, minHeight: 0, display: 'flex', flexDirection: 'column', background: t.sidebarBg, borderLeft: `0.5px solid ${t.border}` }}>
      <InspectorHeader title={tabLabel(active, bridge)} subtitle={subtitle} onClose={() => bridge.setRuntimeInspectorOpen(false)} />
      <InspectorTabs bridge={bridge} />
      <div id="runtime-inspector-panel" role="tabpanel" aria-labelledby={`runtime-inspector-tab-${encodeURIComponent(runtimeCenterItemKey(active))}`} style={{ minHeight: 0, flex: 1, display: 'flex', flexDirection: 'column' }}>
        {active.kind === 'task' && <TaskDetail task={task} bridge={bridge} />}
        {active.kind === 'agent' && <AgentDetail agent={agent} bridge={bridge} />}
        {active.kind === 'resource' && <ResourceDetail resource={resource} bridge={bridge} />}
        {active.kind === 'plan' && <PlanDetail selected={selectedPlan} plan={plan} />}
      </div>
    </aside>
  );
}

export function RuntimeCenter({ bridge }: { bridge: UseBridge }) {
  return <><RuntimeCenterOverview bridge={bridge} /><RuntimeCenterInspector bridge={bridge} /></>;
}

export function runtimeCenterButtonStyle(t: ReturnType<typeof useT>, active: boolean): CSSProperties {
  return { width: 30, height: 30, display: 'grid', placeItems: 'center', borderRadius: 7, border: `0.5px solid ${active ? t.accentBorder : t.border}`, background: active ? t.accentBg : 'transparent', color: active ? t.accent : t.text3, cursor: 'pointer' };
}
