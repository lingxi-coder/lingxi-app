import type { DeepLinkTarget } from '../../../../packages/deep-links/index';
import type { UseBridge } from './bridge/bridgeTypes';
import type { RuntimeCenterItemRef } from './bridge/runtimeCenterState';
import type { SubmittedPlan } from './bridge/submittedPlan';
import { planRuntimeItemId } from './bridge/runtimeCenterState';

export interface SettingsLinkRequest {
  requestId: string;
  kind: 'mcp' | 'skill' | 'plugin' | 'provider';
  id?: string;
  scope?: string;
  payload?: Record<string, unknown>;
}
export interface ScheduledLinkRequest { requestId: string; kind: 'job' | 'run'; id: string; scope?: string }

export function settingsLinkRequest(target: DeepLinkTarget, requestId: string): { pageId: string; linkRequest: SettingsLinkRequest } | null {
  const [kind, entity, id] = target.route.split('/');
  if (kind !== 'extensions' && kind !== 'import') return null;
  const pages: Record<string, string> = { mcp: 'mcp', skill: 'skills', plugin: 'plugins', provider: 'custom-providers' };
  const pageId = pages[entity!];
  if (!pageId) return null;
  if (kind === 'import' && !target.payload) throw new Error('This import link has no configuration to preview.');
  return { pageId, linkRequest: { requestId, kind: entity as SettingsLinkRequest['kind'], id, scope: target.params.scope, payload: target.payload } };
}

/** Resolve against current local data. Missing identifiers never become a generic section. */
export function runtimeLinkItem(kind: string, id: string, bridge: Pick<UseBridge, 'runtimeCenter' | 'desktop' | 'conversation'>, plans: readonly SubmittedPlan[]): RuntimeCenterItemRef | 'activity' {
  if (kind === 'activity' && id === bridge.conversation.sessionKey && id) return 'activity';
  if (kind === 'agent' && bridge.runtimeCenter.agents[id]) return { kind: 'agent', id };
  if (kind === 'task' && bridge.desktop.tasks[id]) return { kind: 'task', id };
  if (kind === 'plan') {
    const document = plans.find(plan => plan.id === id);
    if (document) return { kind: 'plan-document', id: document.id };
    const plan = bridge.runtimeCenter.plan;
    const index = plan.findIndex((task, index) => task.id === id || planRuntimeItemId(task, index, plan) === id);
    if (index >= 0) return { kind: 'plan', id: planRuntimeItemId(plan[index]!, index, plan) };
  }
  if (kind === 'review') {
    const turn = bridge.conversation.items.find(item => item.type === 'meta' && item.id === id);
    if (turn?.type === 'meta' && turn.files?.length) return { kind: 'turn-review', id, files: turn.files };
    if (id === 'workspace') return { kind: 'section', id: 'review' };
  }
  throw new Error(`This ${kind} is unavailable in the current local conversation.`);
}

export function terminalCommandInput(command: string): string {
  if (!command.trim() || /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/u.test(command)) throw new Error('Enter a command without terminal control characters.');
  // Bracketed paste prevents embedded line breaks from executing part of a reviewed command.
  return `\x1b[200~${command.replace(/\r\n?/g, '\n')}\x1b[201~\r`;
}
