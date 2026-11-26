import { useCallback, useEffect, useRef, useState } from 'react';
import type { ExecutionScope } from './execution.js';
import type { TeamsApi } from './index.js';
import { approveHeldReply, HeldRepliesBoard, readHeldRepliesOf, rejectHeldReply, type HeldView } from './held-replies.js';

/** What a screen that has no replies to show says. */
const IDLE: HeldView = { status: 'ready', rows: [], error: undefined, now: 0 };

export interface HeldReplies {
  readonly view: HeldView;
  /** Let the reply go as the Bot made it, or with the maintainer's own text. Whether the service took it. */
  approve(taskId: string, text?: string): Promise<boolean>;
  reject(taskId: string): Promise<boolean>;
  /** The person has read what became of a reply; it goes from the screen. */
  dismiss(taskId: string): void;
  reload(): void;
}

/**
 * The replies of `request.sharedBotIds` that wait for this person, in one Chat or (without `conversationId`) in all of them, kept up to date by a screen. They are read when
 * the screen opens, whenever `watch` changes (the screen says it with whatever tells it the Chat's tasks moved), whenever the service says a Bot changed or a message
 * came to the Chat, and when the first reply runs out; never on a timer of their own, so no oftener than the tasks of the Chat are. Nothing is read while `online` is false;
 * what was read stays, and the list is read again when the connection returns.
 *
 * `request` is absent where this person maintains no such Bot, and then nothing is read. A change of Team, Chat or Bots starts the list over, and what was read for the old
 * one is never shown for the new one. `onDecided` is called after the service took a decision, so that the screen can fetch what it changed.
 */
export function useHeldReplies(api: Pick<TeamsApi, 'messages' | 'onMessagesChanged'>, scope: ExecutionScope, request: { sharedBotIds: readonly string[]; conversationId?: string } | undefined,
  options: { online?: boolean; watch?: string; onDecided?(): void } = {}): HeldReplies {
  const online = options.online ?? true, watch = options.watch ?? '';
  const key = request?.sharedBotIds.length ? `${scope.teamId}:${scope.generation}:${request.conversationId ?? ''}:${request.sharedBotIds.join(',')}` : '';
  // The list says whose it is, so that for the render in which the Chat or the Bots changed it is still the old one and is not taken for the new one.
  const [state, setState] = useState<{ key: string; view: HeldView }>({ key: '', view: IDLE });
  const board = useRef<HeldRepliesBoard>();
  const decided = useRef(options.onDecided);
  decided.current = options.onDecided;
  // What the list was last read for: a change of it, or the connection coming back, is a reason to read it again.
  const seen = useRef({ watch, online });
  const onlineNow = useRef(online);
  onlineNow.current = online;
  useEffect(() => {
    if (!key || !request) return undefined;
    const bots = request.sharedBotIds, conversationId = request.conversationId;
    const next = new HeldRepliesBoard(() => readHeldRepliesOf(api, scope, bots, conversationId),
      { approve: (taskId, options) => approveHeldReply(api, scope, taskId, options), reject: (taskId, options) => rejectHeldReply(api, scope, taskId, options) },
      view => setState({ key, view }));
    board.current = next;
    next.start();
    if (onlineNow.current) void next.load();
    const stop = api.onMessagesChanged(signal => { if (onlineNow.current && (!signal || signal.bots || (conversationId !== undefined && signal.conversationId === conversationId))) void next.load(); });
    return () => { stop(); next.close(); if (board.current === next) board.current = undefined; };
  // `key` is what `scope` and `request` say.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, key]);
  useEffect(() => {
    const before = seen.current;
    seen.current = { watch, online };
    if (online && (before.watch !== watch || !before.online)) void board.current?.load();
  }, [watch, online]);
  const approve = useCallback(async (taskId: string, text?: string) => {
    const taken = await board.current?.approve(taskId, text) ?? false;
    if (taken) decided.current?.();
    return taken;
  }, []);
  const reject = useCallback(async (taskId: string) => {
    const taken = await board.current?.reject(taskId) ?? false;
    if (taken) decided.current?.();
    return taken;
  }, []);
  const dismiss = useCallback((taskId: string) => board.current?.dismiss(taskId), []);
  const reload = useCallback(() => void board.current?.load(), []);
  return { view: state.key === key && key ? state.view : IDLE, approve, reject, dismiss, reload };
}
