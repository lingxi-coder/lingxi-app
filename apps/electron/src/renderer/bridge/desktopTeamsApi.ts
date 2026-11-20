import type { InboxController } from '../../../../../packages/inbox/controller';
import { inboxKey, type InboxMutation } from '../../../../../packages/inbox/contracts';
import { browserInboxStorage, enqueue } from '../../../../../packages/inbox/storage';
import type { DesktopInboxApi } from '../../shared/inbox';
import { DESKTOP_BRIDGE_GLOBAL } from '../../shared/desktopBridge';
import type { DesktopTeamsApi, TeamsApi } from '../../../../../packages/teams';

/** The two switches of the Bots page that keep this Mac serving, as the Teams pages change them. */
export interface HostBackground {
  setKeepAwake(enabled: boolean): Promise<unknown>;
  setOpenAtLogin(enabled: boolean): Promise<unknown>;
  openLoginItems(): Promise<unknown>;
}

const readers = new Map<string, Pick<InboxController, 'mutate'>>();
export function registerDesktopInboxReads(uid: string | null, controller: Pick<InboxController, 'mutate'>) {
  if (uid) readers.set(uid, controller);
  return () => {
    if (uid && readers.get(uid) === controller) readers.delete(uid);
  };
}

async function recordVisible(uid: string, command: InboxMutation) {
  const controller = readers.get(uid);
  if (controller) return controller.mutate(command);
  // The selected conversation may mount before the inbox effect. Its account-scoped intent drains on start.
  await browserInboxStorage.update(uid, (state) => enqueue(state, command, crypto.randomUUID()));
}

/** The renderer's `TeamsApi` over the preload bridge, for one account. */
export function desktopTeamsApi(
  desktop: DesktopTeamsApi,
  uid: string,
  background?: HostBackground,
  inbox: DesktopInboxApi | undefined = typeof window === 'undefined' ? undefined : window[DESKTOP_BRIDGE_GLOBAL]?.inbox,
): TeamsApi {
  return {
    hostStatus: () => desktop.hostStatus(uid),
    ...(background
      ? {
          setHostBackground: async (change) => {
            if (change.openAtLogin !== undefined) await background.setOpenAtLogin(change.openAtLogin);
            if (change.keepAwake !== undefined) await background.setKeepAwake(change.keepAwake);
          },
          openLoginItems: async () => {
            await background.openLoginItems();
          },
        }
      : {}),
    localBots: () => desktop.localBots(uid),
    selectSharedFiles: (instanceId) => desktop.selectSharedFiles(uid, instanceId),
    publishedKnowledge: (teamId, sharedBotId) => desktop.publishedKnowledge(uid, teamId, sharedBotId),
    publishLocalBot: (input) => desktop.publishLocalBot(uid, input),
    offerSharedBot: (input) => desktop.offerSharedBot(uid, input),
    acceptSharedBot: (input) => desktop.acceptSharedBot(uid, input),
    openBotMarket: () => desktop.openBotMarket(uid),
    list: () => desktop.list(uid),
    detail: (teamId) => desktop.detail(uid, teamId),
    execute: (command) => desktop.execute(uid, command),
    ...(desktop.historyPage
      ? {
          historyPage: (scope, conversationId, beforeSeq) =>
            desktop.historyPage!(uid, scope, conversationId, beforeSeq),
        }
      : {}),
    ...(inbox
      ? {
          readVisible: (
            scope,
            id,
            through,
            manualUnreadRevision,
            observedPendingActionRevision,
            observedFailureRevision,
          ) =>
            recordVisible(uid, {
              action: 'markRead',
              key: inboxKey({ kind: 'team', sourceId: scope.teamId, conversationId: id }),
              generation: scope.generation,
              through,
              manualUnreadRevision,
              ...(observedPendingActionRevision === undefined ? {} : { observedPendingActionRevision }),
              ...(observedFailureRevision === undefined ? {} : { observedFailureRevision }),
              operationId: crypto.randomUUID(),
            }),
          peerReceipt: (scope, id) =>
            inbox.request<{ readThrough: number | null }>(uid, {
              action: 'peerReceipt',
              key: inboxKey({ kind: 'team', sourceId: scope.teamId, conversationId: id }),
              generation: scope.generation,
            }),
        }
      : {}),
    watchMessages: (scope, conversationId, next) => desktop.watchMessages(uid, scope, conversationId, next),
    services: (command) => desktop.services(uid, command),
    serviceMessages: (command) => desktop.serviceMessages(uid, command),
    data: (request) => desktop.data(uid, request),
    messages: (command) => desktop.messages(uid, command),
    onMessagesChanged: (callback) => desktop.onMessagesChanged(callback),
    onChanged: (callback) => desktop.onChanged(callback),
  };
}
