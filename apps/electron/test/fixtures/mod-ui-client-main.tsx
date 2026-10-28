import * as React from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import type { AllowedClientCommand } from '../../src/shared/clientCommands.js';
import type {
  NativeUiControlRequest,
  NativeUiControlResponseFor,
  NativeUiRenderResponseDto,
  UiClientFrameDto,
  UiClientFrameEventDto,
  UiClientFrameEventSnapshotDto,
  UiClientOperation,
  UiClientOperationResponseFor,
  UiControlCallResultDto,
  UiInvalidateEventDto,
} from '@lingxi/bridge-client';
import {
  ModUiAbovePrompt,
  ModUiParentSite,
  ModUiSurfaceClientIdProvider,
  type ModUiClientHostApi,
} from '../../src/renderer/components/modUiAbovePrompt';
import { BetaComposer } from '../../src/renderer/components/BetaDesktop';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { UiSurfaceLifecycleQueue } from '../../src/renderer/bridge/uiSurfaceLifecycle';
import { emptyConversation } from '../../src/renderer/bridge/conversation';
import { emptyRuntimeCenterState } from '../../src/renderer/bridge/runtimeCenterState';
import { defaultNativeAudioSnapshot } from '../../src/shared/nativeAudio';
import { contextMentionHref as buildContextMentionHref, OPEN_CONTEXT_MENTION_EVENT } from '../../src/renderer/bridge/composerMentions';

interface FixtureApi {
  calls: Array<({ kind: 'control'; request: NativeUiControlRequest } | { kind: 'operation'; request: UiClientOperation }) & {
    sessionId: string;
    domCommitted?: boolean;
    returnedRevision?: number;
    returnedRuntimeId?: string;
    returnedFrameSequence?: number;
    returnedClientEnvironmentEpoch?: number;
    handled?: boolean;
    workerFault?: boolean;
  }>;
  contextMentionHref(): string;
  contextMentionEvents(): unknown[];
  clientIdentitySnapshot(): {
    attachedClientIds: string[];
    primaryRenderClientId?: string;
    probeRenderClientId?: string;
    parentControlClientIds: string[];
  };
  debugSnapshot(): unknown;
  updateParentProps(title: string): void;
  rerenderParentWithoutClientChange(): void;
  advanceClientStateToken(): void;
  unloadStatusPlugin(): void;
  loadStatusPlugin(): void;
  emitStaleFrame(): void;
  rejectNextRunAtAdapter(): void;
  ignoreNextRunAsStale(): void;
  failNextRunInWorker(): void;
  showNoMods(): void;
  disable(): void;
  startLateRender(): void;
  requestLateRerender(): void;
  releaseLateRenderAt(index: number): void;
  disableLateRender(): void;
  holdNextFrameOperation(): void;
  releaseHeldFrameOperations(): void;
  initialStatusResizeHeld(): boolean;
  initialMountsHeld(): boolean;
  releaseInitialMounts(): void;
  emitNewerAsyncFrame(): void;
  emitHighSequenceFrameForFailedRuntime(runtimeId: string): void;
  reloadStatusPluginEnvironment(): void;
  emitAsyncWorkerFault(): void;
  enableAsyncFaultClient(): void;
  disableAsyncFaultClient(): void;
  startRapidInputScenario(mode?: RapidInputMode): void;
  setRapidInputMode(mode: RapidInputMode): void;
  holdRapidPostAt(ordinal: number): void;
  releaseRapidPostAt(ordinal: number): void;
  ignoreNextRapidRunAsStale(): void;
  emitRapidExternalValue(value: string | undefined): void;
  rapidInputSnapshot(): unknown;
  raceNextRapidAck(value: string): void;
  captureRapidInput(): void;
  invalidateRapidSite(value: string): void;
  releaseRapidRevisionFrame(): void;
  enableRapidSibling(): void;
  misrouteNextRapidFrame(): void;
  enableRapidGroups(): void;
  swapRapidGroups(): void;
  holdNextRapidControl(): void;
  holdNextRapidRunRejection(): void;
  releaseRapidRunRejection(): void;
  replaceRapidSite(): void;
  releaseRapidControl(): void;
}

type RapidInputMode = 'identity' | 'uppercase' | 'clear-on-submit' | 'uncontrolled';

declare global {
  interface Window { __modUiClientFixture?: FixtureApi }
}

const sessionId = 'mod-ui-client-fixture-session';
const lateSessionId = 'mod-ui-client-late-render-session';
const rapidSessionId = 'mod-ui-client-rapid-input-session';
const primaryClientId = 'fixture-window-client-one';
const probeClientId = 'fixture-window-client-two';
const identityProbeProps = {};
const identityProbeViewport = { columns: 24, rows: 4 };
const calls: FixtureApi['calls'] = [];
const fixtureContextMentionHref = buildContextMentionHref({ kind: 'plugin', name: 'Fixture', target: 'plugin:fixture' });
const contextMentionEvents: unknown[] = [];
window.addEventListener(OPEN_CONTEXT_MENTION_EVENT, (event) => contextMentionEvents.push((event as CustomEvent).detail));
const lifecycleCalls: Array<{ sessionId: string; command: AllowedClientCommand }> = [];
const frameListeners = new Set<(event: UiClientFrameEventDto) => void>();
const invalidationListeners = new Set<(event: UiInvalidateEventDto) => void>();
const runtimeFrames = new Map<string, UiClientFrameDto>();
const frameIdentityByRuntime = new Map<string, Pick<UiClientFrameEventSnapshotDto, 'parent' | 'client'>>();
const frameSequenceByRuntime = new Map<string, number>();
const failedRuntimeIds = new Set<string>();
const heldFrameOperationReleases: Array<() => void> = [];
let revision = 1;
let title = 'Ready';
let clientRuntimeEpochs: Record<string, number> = { 'fixture-plugin': 1 };
let clientStateVersion = 0;
let currentStatusRuntimeId = 'runtime-status';
let rejectRunAtAdapter = false;
let failRunInWorker = false;
let ignoreRunAsStale = false;
let noParent = false;
let statusPluginLoaded = true;
let asyncFaultClientEnabled = false;
let holdInitialResize = true;
let initialStatusMountRelease: (() => void) | null = null;
let initialSiblingMountRelease: (() => void) | null = null;
let holdNextFrame = false;
let nextRuntimeId = 1;
let setRapidInputEnabled!: (enabled: boolean) => void;
let rapidInputValue = '';
let rapidPropValue: string | undefined = '';
let rapidInputMode: RapidInputMode = 'identity';
let rapidInputResult = 'Waiting for a value.';
let rapidControlOrdinal = 0;
let rapidPostOrdinal = 0;
let rapidNextHandle = 500;
let rapidInputHandle = 0;
let rapidButtonHandle = 0;
let rapidButtonSubmitCount = 0;
let rapidInputSubmitCount = 0;
let mainPromptSubmitCount = 0;
const rapidHeldPostOrdinals = new Set<number>();
const rapidHeldPostReleases = new Map<number, () => void>();
const rapidHandleKinds = new Map<number, 'input' | 'button'>();
const rapidInputCallbacks: Array<{ kind: string; value: string }> = [];
const rapidOnInputCallbacks: string[] = [];
const rapidOnSubmitCallbacks: string[] = [];
const rapidButtonCallbacks: string[] = [];
const rapidBackgroundPending: Array<{ ordinal: number; value: string | undefined }> = [];
let rapidIgnoreNextRunAsStale = false;
let rapidNextAckExternalValue: string | undefined;
let rapidAckRaceFired = false;
let rapidRevision = 200;
let holdRapidRevisionFrame = false;
let rapidRevisionFrameRelease: (() => void) | null = null;
let rapidNextRevisionValue = '';
let capturedRapidInput: HTMLInputElement | null = null;
let rapidSiblingEnabled = false;
let rapidMisrouteNextFrame = false;
let rapidGroupsEnabled = false;
let rapidGroupsReversed = false;
let rapidSiteInstanceId = 'rapid-input-site';
let holdNextRapidControl = false;
let rapidControlRelease: (() => void) | null = null;
let holdNextRapidRunRejection = false;
let rapidRunRejectionRelease: (() => void) | null = null;
let rapidRunRejections = 0;
const statusMountCountsByEpoch = new Map<number, number>();
let asyncFaultMountCount = 0;
let nextLateRevision = 100;
let latestStartedLateRevision: number | null = null;
let currentLateRevision: number | null = null;
const lateRenderResolvers: Array<{ revision: number; resolve: () => void }> = [];
let setEnabled!: (enabled: boolean) => void;
let setLateEnabled!: (enabled: boolean) => void;

const lifecycleHost = {
  async command(targetSessionId: string, command: AllowedClientCommand): Promise<void> {
    lifecycleCalls.push({ sessionId: targetSessionId, command });
  },
};

function latestAbovePromptRenderCall(session = sessionId) {
  return [...calls].reverse().find((call) => call.kind === 'control'
    && call.sessionId === session
    && call.request.subtype === 'ui_render'
    && call.request.component === 'AbovePrompt');
}

const parentTree = (): NativeUiRenderResponseDto['tree'] => ({
  type: 'Box',
  children: [
    ...(statusPluginLoaded ? [{ type: 'Client', props: { key: 'status', module: 'surface/status.tsx', props: { title } }, client: { plugin: 'fixture-plugin' } }] : []),
    ...(asyncFaultClientEnabled ? [{ type: 'Client', props: { key: 'async-fault', module: 'surface/async-fault.tsx', props: {} }, client: { plugin: 'fixture-plugin' } }] : []),
    { type: 'Client', props: { key: 'sibling', module: 'surface/sibling.tsx', props: {} }, client: { plugin: 'fixture-plugin' } },
    { type: 'Button', props: { key: 'parent-run', label: 'Parent Run', hotkey: 'r' }, press: { plugin: 'fixture-plugin', handle: 31 } },
    { type: 'Input', props: { key: 'parent-filter', label: 'Parent Filter', value: '', submitLabel: 'Apply' }, press: { plugin: 'fixture-plugin', handle: 32 } },
    { type: 'Select', props: { key: 'parent-period', label: 'Parent Period', options: [{ value: 'day', label: 'Day' }, { value: 'week', label: 'Week' }], value: 'day' }, press: { plugin: 'fixture-plugin', handle: 33 } },
    { type: 'Link', props: { href: 'https://example.com/parent', label: 'Parent Link' } },
    { type: 'Code', props: { source: 'const parentReady = true;', language: 'typescript', path: 'parent.ts', startLine: 4 } },
    { type: 'Markdown', props: { key: 'parent-markdown', text: '[Parent docs](https://example.com/docs)', pressableLinks: ['https://example.com/docs'] }, press: { plugin: 'fixture-plugin', handle: 34 } },
    { type: 'Svg', props: { source: '<svg xmlns="http://www.w3.org/2000/svg"><title>Parent diagram</title><circle cx="5" cy="5" r="4"/></svg>', alt: 'Parent diagram', width: 24, height: 24 } },
    { type: 'engine', ref: 0 },
  ],
});

const surfaceTree = (displayTitle = title): UiClientFrameDto['tree'] => ({
  type: 'Box',
  props: { flexDirection: 'column', gap: 6 },
  children: [
    { type: 'Text', props: { key: 'status-text', bold: true }, children: [displayTitle] },
    { type: 'Button', props: { key: 'run', label: 'Run' }, press: { plugin: 'fixture-plugin', handle: 17 } },
    { type: 'Input', props: { key: 'filter', label: 'Filter', value: '' }, press: { plugin: 'fixture-plugin', handle: 18 } },
    { type: 'Select', props: { key: 'period', label: 'Period', options: [{ value: 'day' }, { value: 'week' }] }, press: { plugin: 'fixture-plugin', handle: 19 } },
    { type: 'Markdown', props: { key: 'docs', text: `[Docs](https://example.test/docs) [Plugin mention](${fixtureContextMentionHref}) [Other link](https://example.test/private)`, pressableLinks: ['https://example.test/docs', fixtureContextMentionHref] } },
    { type: 'Markdown', props: { key: 'open-links', text: '[Open link](https://example.test/open)' } },
    { type: 'Markdown', props: { key: 'ordinary-mention', text: `[Ordinary mention](${fixtureContextMentionHref})` } },
  ],
});

function rapidSurfaceTree(): UiClientFrameDto['tree'] {
  rapidHandleKinds.clear();
  rapidInputHandle = ++rapidNextHandle;
  rapidButtonHandle = ++rapidNextHandle;
  rapidHandleKinds.set(rapidInputHandle, 'input');
  rapidHandleKinds.set(rapidButtonHandle, 'button');
  const input: UiJsonValue = { type: 'Input', props: { key: 'rapid-message', label: 'Message', ...(rapidPropValue === undefined ? {} : { value: rapidPropValue }) }, press: { plugin: 'fixture-plugin', handle: rapidInputHandle } };
  const button: UiJsonValue = { type: 'Button', props: { key: 'rapid-submit', label: 'Submit' }, press: { plugin: 'fixture-plugin', handle: rapidButtonHandle } };
  const status: UiJsonValue = { type: 'Text', children: [rapidInputResult] };
  const inputGroup: UiJsonValue = { type: 'Box', props: { key: 'rapid-input-group' }, children: [input] };
  const controlGroup: UiJsonValue = { type: 'Box', props: { key: 'rapid-control-group' }, children: [button, status] };
  return {
    type: 'Box',
    props: { flexDirection: 'column', gap: 6 },
    children: rapidGroupsEnabled
      ? rapidGroupsReversed ? [controlGroup, inputGroup] : [inputGroup, controlGroup]
      : [input, button, status],
  };
}

function rapidParentTree(): NativeUiRenderResponseDto['tree'] {
  return {
    type: 'Box',
    children: [
      { type: 'Client', props: { key: 'rapid-input', module: 'surface/rapid-input.tsx', props: {} }, client: { plugin: 'fixture-plugin' } },
      ...(rapidSiblingEnabled ? [{ type: 'Client', props: { key: 'rapid-sibling', module: 'surface/rapid-sibling.tsx', props: {} }, client: { plugin: 'fixture-plugin' } }] : []),
    ],
  };
}

const noop = async () => undefined;
const composerBridge: any = {
  uiSurfaceClientId: 'fixture-composer-client', hosted: true, loading: false, connected: true, sessionLoading: false,
  activeSession: undefined,
  bootstrap: { settings: { projects: [], activeProject: null, activeSession: null }, workspace: { path: '/tmp', trusted: false }, activeSession: null, runtimes: [], projectCatalogs: {}, connection: { status: 'connected' }, diagnostics: [] },
  settingsSnapshotEvent: null, mcpServersEvent: null, skillsEvent: null, skillCatalogEvent: null, skillDocumentEvent: null,
  mcpConfigurationSnapshotEvent: null, fusionCatalogEvent: null, pluginCatalogEvent: null, configurationOperations: {},
  desktop: { sessions: [], activeSessionId: null, models: [], modelDetails: [], currentModel: 'openai/gpt-4o', currentModelSource: null,
    conversationControls: null, fastMode: false, permissionMode: 'default', slashCommands: [], tasks: {}, taskOutput: {}, status: null, doctor: null },
  conversation: { ...emptyConversation(), sessionKey: '' }, runtimeCenter: emptyRuntimeCenterState(), usage: null,
  running: false, isCancelling: false, pendingPermission: null, pendingComputerAccess: null, pendingAskUserQuestion: null,
  audioSnapshot: defaultNativeAudioSnapshot(), audioRealtimeState: { phase: 'idle' },
  updatePromptSuggestionContext: noop, promptSuggestionFeedback: noop,
  audioRealtimeCommand: noop, audioExecute: noop, audioCancel: noop, audioFinishListen: noop,
  refreshSlashCommands: noop, refreshModelPicker: noop, refreshSkills: noop, skillAdmin: noop, pluginAdmin: noop,
  searchWorkspaceFiles: async () => ({ files: [], directories: [], truncated: false }), previewWorkspaceFile: noop,
  setModel: noop, setReasoningSelection: noop, setFastMode: noop, setPermissionMode: noop, emitCommandOutput: noop,
  beginLocalCommand: noop, runSlashCommand: noop, sendPrompt: async () => { mainPromptSubmitCount += 1; }, cancel: noop,
};

function makeRuntimeFrame(runtimeId: string, renderRevision: number, tree: UiClientFrameDto['tree']): UiClientFrameDto {
  const frameSequence = (frameSequenceByRuntime.get(runtimeId) ?? 0) + 1;
  frameSequenceByRuntime.set(runtimeId, frameSequence);
  const frame = { runtimeId, renderRevision, frameSequence, tree, hasPointerListener: false, hasKeyListener: false };
  runtimeFrames.set(runtimeId, frame);
  return frame;
}

function emitRuntimeFrame(sessionId: string, frame: UiClientFrameDto): void {
  const { runtimeId, ...frameBody } = frame;
  const identity = frameIdentityByRuntime.get(runtimeId);
  if (!identity) throw new Error(`missing parent/client identity for runtime ${runtimeId}`);
  for (const listener of frameListeners) listener({ sessionId, runtimeId, frame: { ...frameBody, ...identity } });
}

function emitRapidBackgroundFrame(value: string | undefined, renderRevision = rapidRevision): void {
  rapidPropValue = value;
  emitRuntimeFrame(rapidSessionId, makeRuntimeFrame('runtime-rapid-input', renderRevision, rapidSurfaceTree()));
}

function emitSnapshotFrame(
  sessionId: string,
  runtimeId: string,
  frame: Omit<UiClientFrameEventSnapshotDto, 'parent' | 'client'>,
): void {
  const identity = frameIdentityByRuntime.get(runtimeId);
  if (!identity) throw new Error(`missing parent/client identity for runtime ${runtimeId}`);
  for (const listener of frameListeners) listener({ sessionId, runtimeId, frame: { ...frame, ...identity } });
}

function emitWorkerFaultFrame(sessionId: string, runtimeId: string, renderRevision: number, reason: string): void {
  for (const listener of frameListeners) listener({
    sessionId,
    runtimeId,
    frame: { renderRevision, fault: { phase: 'run', reason, source: 'worker' } },
  });
}

function holdFrameResult(frame: UiClientFrameDto): Promise<UiClientFrameDto> {
  return new Promise((resolve) => heldFrameOperationReleases.push(() => resolve(frame)));
}

const host: ModUiClientHostApi = {
  async control<T extends NativeUiControlRequest>(session: string, request: T): Promise<UiControlCallResultDto<NativeUiControlResponseFor<T>>> {
    if (session !== sessionId && session !== lateSessionId && session !== rapidSessionId) throw new Error('unexpected session');
    const call: FixtureApi['calls'][number] = { kind: 'control', sessionId: session, request };
    calls.push(call);
    if (session === lateSessionId && request.subtype === 'ui_render') {
      const returnedRevision = nextLateRevision++;
      latestStartedLateRevision = returnedRevision;
      return await new Promise((resolve) => {
        lateRenderResolvers.push({ revision: returnedRevision, resolve: () => {
          call.returnedRevision = returnedRevision;
          if (latestStartedLateRevision === returnedRevision) currentLateRevision = returnedRevision;
          resolve({
            response: { tree: { type: 'Text', children: [`Late revision ${returnedRevision}`] }, props: {}, rewritten: false, hooked: false },
            metadata: { renderRevision: returnedRevision },
          } as UiControlCallResultDto<NativeUiControlResponseFor<T>>);
        } });
      });
    }
    let response: unknown;
    let metadata: { renderRevision: number; clientRuntimeEpochs?: Record<string, number>; clientStateToken?: string } | undefined;
    if (session === rapidSessionId && request.subtype === 'ui_render') {
      response = { tree: rapidParentTree(), props: {}, rewritten: false, hooked: false } satisfies NativeUiRenderResponseDto;
      metadata = { renderRevision: rapidRevision, clientRuntimeEpochs: { 'fixture-plugin': 1 }, clientStateToken: 'rapid-state-1' };
      call.returnedRevision = metadata.renderRevision;
    } else if (session === rapidSessionId && request.subtype === 'ui_client_press') {
      rapidControlOrdinal += 1;
      if (holdNextRapidControl) {
        holdNextRapidControl = false;
        await new Promise<void>((resolve) => { rapidControlRelease = resolve; });
      }
      response = request.event.type === 'input'
        ? { handled: true, reached: { element: request.element, value: request.event.value } }
        : { handled: true, reached: { element: request.element } };
    } else if (request.subtype === 'ui_render') {
      const identityProbe = request.instance_id === 'identity-probe-client-two';
      response = identityProbe
        ? { tree: { type: 'Box', children: [] }, props: {}, rewritten: false, hooked: false } satisfies NativeUiRenderResponseDto
        : noParent
          ? { tree: null, props: {}, rewritten: false, hooked: false } satisfies NativeUiRenderResponseDto
          : { tree: parentTree(), props: { engineLabel: 'Response engine row' }, rewritten: false, hooked: false } satisfies NativeUiRenderResponseDto;
      if (!noParent || identityProbe) metadata = {
        renderRevision: revision,
        clientRuntimeEpochs: { ...clientRuntimeEpochs },
        ...(!identityProbe && !noParent ? { clientStateToken: String(clientStateVersion) } : {}),
      };
      call.returnedRevision = metadata?.renderRevision;
    } else if (request.subtype === 'ui_client_press') {
      if (request.event.type === 'press') response = { handled: true, reached: { element: request.element } };
      else if (request.event.type === 'input') response = { handled: true, reached: { element: 'filter', value: request.event.kind === 'submit' ? 'normalized-submitted-input' : 'normalized-input' } };
      else response = { handled: true, reached: { element: 'period', value: 'week' } };
    } else if (request.subtype === 'ui_client_fault') {
      response = { handled: true };
    } else if (request.subtype === 'ui_press') {
      response = { handled: true, element: request.key };
    } else if (request.subtype === 'ui_input' || request.subtype === 'ui_select') {
      response = { handled: true, element: request.key, value: request.value };
    } else {
      response = { handled: false };
    }
    return { response, ...(metadata ? { metadata } : {}) } as UiControlCallResultDto<NativeUiControlResponseFor<T>>;
  },
  async operation<T extends UiClientOperation>(session: string, request: T): Promise<UiClientOperationResponseFor<T>> {
    if (session !== sessionId && session !== lateSessionId && session !== rapidSessionId) throw new Error('unexpected session');
    const call: FixtureApi['calls'][number] = {
      kind: 'operation',
      sessionId: session,
      request,
      ...(request.type === 'draw_commit' ? { domCommitted: Boolean(document.querySelector('.mod-ui-client-instance')) } : {}),
    };
    calls.push(call);
    if (session === lateSessionId && (request.type === 'draw_commit' || request.type === 'draw_unmount')) {
      const handled = request.render_revision === currentLateRevision
        && request.render_revision === latestStartedLateRevision;
      if (request.type === 'draw_unmount' && handled) {
        currentLateRevision = null;
        latestStartedLateRevision = null;
      }
      return { handled, renderRevision: request.render_revision } as UiClientOperationResponseFor<T>;
    }
    if (request.type === 'draw_commit' || request.type === 'draw_unmount' || request.type === 'unmount') {
      call.returnedRevision = request.render_revision;
      call.handled = true;
      return { handled: true, renderRevision: request.render_revision } as UiClientOperationResponseFor<T>;
    }
    if (request.type === 'mount') {
      const statusEpoch = clientRuntimeEpochs[request.plugin] ?? 1;
      call.returnedClientEnvironmentEpoch = statusEpoch;
      let runtimeId: string;
      if (request.client === 'rapid-input') {
        runtimeId = 'runtime-rapid-input';
      } else if (request.client === 'rapid-sibling') {
        runtimeId = 'runtime-rapid-sibling';
      } else if (request.client === 'status') {
        const mountCount = (statusMountCountsByEpoch.get(statusEpoch) ?? 0) + 1;
        statusMountCountsByEpoch.set(statusEpoch, mountCount);
        runtimeId = mountCount === 1
          ? statusEpoch === 1 ? 'runtime-status' : `runtime-status-epoch-${statusEpoch}`
          : `runtime-status-epoch-${statusEpoch}-mount-${mountCount}`;
      } else if (request.client === 'async-fault') {
        asyncFaultMountCount += 1;
        runtimeId = asyncFaultMountCount === 1 ? 'runtime-async-fault' : `runtime-async-fault-mount-${asyncFaultMountCount}`;
      } else {
        runtimeId = `runtime-sibling-${nextRuntimeId++}`;
      }
      call.returnedRuntimeId = runtimeId;
      frameIdentityByRuntime.set(runtimeId, {
        parent: { surface: request.surface, component: request.component, instance_id: request.instance_id },
        client: { plugin: request.plugin, key: request.client, module: request.module },
      });
      if (request.client === 'status') {
        currentStatusRuntimeId = runtimeId;
      }
      const mountTree = request.client === 'rapid-input'
        ? rapidSurfaceTree()
        : request.client === 'rapid-sibling' ? { type: 'Text', children: ['Rapid sibling alive'] }
        : request.client === 'status'
        ? surfaceTree(statusEpoch === 1 ? 'Mount response frame' : 'Recovered after environment remount')
        : request.client === 'async-fault'
          ? { type: 'Text', children: ['Async mount response frame'] }
          : { type: 'Text', children: ['Sibling Alive'] };
      const mountFrame = makeRuntimeFrame(runtimeId, request.render_revision, mountTree);
      if (request.client === 'status' && statusEpoch === 1) {
        emitRuntimeFrame(session, makeRuntimeFrame(runtimeId, request.render_revision, surfaceTree('Scheduled first frame')));
        emitRuntimeFrame(session, makeRuntimeFrame(runtimeId, request.render_revision, surfaceTree('Scheduled final frame')));
      }
      if (request.client === 'async-fault') {
        emitRuntimeFrame(session, makeRuntimeFrame(runtimeId, request.render_revision, { type: 'Text', children: ['Buffered pre-fault frame'] }));
        failedRuntimeIds.add(runtimeId);
        emitWorkerFaultFrame(session, runtimeId, request.render_revision, 'fixture pre-registration worker fault');
      }
      // Hold both first responses across one controlled viewport update. This
      // makes their stale cleanup deterministic even on slow hosts.
      if (session === sessionId && runtimeId === 'runtime-status') {
        await new Promise<void>(resolve => { initialStatusMountRelease = resolve; });
      } else if (session === sessionId && runtimeId === 'runtime-sibling-1') {
        await new Promise<void>(resolve => { initialSiblingMountRelease = resolve; });
      }
      return mountFrame as UiClientOperationResponseFor<T>;
    }
    if (request.type === 'runHeld' && failRunInWorker) {
      failRunInWorker = false;
      call.workerFault = true;
      failedRuntimeIds.add(request.runtimeId);
      return {
        handled: false,
        renderRevision: request.render_revision,
        runtimeId: request.runtimeId,
        fault: { phase: 'run', reason: 'fixture worker callback failed', source: 'worker' },
      } as UiClientOperationResponseFor<T>;
    }
    if (request.type === 'runHeld' && ignoreRunAsStale) {
      ignoreRunAsStale = false;
      return { handled: false, renderRevision: request.render_revision } as UiClientOperationResponseFor<T>;
    }
    if (request.type === 'runHeld' && rejectRunAtAdapter) {
      rejectRunAtAdapter = false;
      throw new Error('fixture callback failed');
    }
    if (session === rapidSessionId && request.type === 'runHeld' && holdNextRapidRunRejection) {
      holdNextRapidRunRejection = false;
      await new Promise<void>((resolve) => { rapidRunRejectionRelease = resolve; });
      rapidRunRejections += 1;
      throw new Error('fixture delayed stale run failure');
    }
    if (session === rapidSessionId && request.type === 'runHeld' && rapidIgnoreNextRunAsStale) {
      rapidIgnoreNextRunAsStale = false;
      call.handled = false;
      return { handled: false, renderRevision: request.render_revision } as UiClientOperationResponseFor<T>;
    }
    if (session === rapidSessionId && request.runtimeId === 'runtime-rapid-input' && request.type === 'runHeld') {
      const kind = rapidHandleKinds.get(request.handle);
      if (kind === 'input') {
        const event = request.event as { kind?: string; value?: unknown } | undefined;
        const value = typeof event?.value === 'string' ? event.value : '';
        rapidInputCallbacks.push({ kind: typeof event?.kind === 'string' ? event.kind : 'unknown', value });
        if (event?.kind === 'submit') {
          rapidOnSubmitCallbacks.push(value);
          rapidInputSubmitCount += 1;
          if (rapidInputMode === 'clear-on-submit') rapidInputValue = '';
        } else {
          rapidOnInputCallbacks.push(value);
          rapidInputValue = rapidInputMode === 'uppercase' ? value.toUpperCase() : value;
        }
        const post = {
          ordinal: ++rapidPostOrdinal,
          value: rapidInputMode === 'uncontrolled' ? undefined : rapidInputValue,
        };
        rapidBackgroundPending.push(post);
        if (rapidHeldPostOrdinals.has(post.ordinal)) {
          await new Promise<void>((resolve) => rapidHeldPostReleases.set(post.ordinal, resolve));
        }
        const settledPostIndex = rapidBackgroundPending.findIndex((pending) => pending.ordinal === post.ordinal);
        if (settledPostIndex !== -1) rapidBackgroundPending.splice(settledPostIndex, 1);
        // Model the independent ui.message path settling before Host returns
        // the authoritative follow-up runHeld frame.
        emitRapidBackgroundFrame(post.value, request.render_revision);
      } else if (kind === 'button') {
        rapidButtonCallbacks.push(rapidInputValue);
        rapidButtonSubmitCount += 1;
        rapidInputResult = `Button accepted: ${rapidInputValue}`;
      } else {
        call.handled = false;
      }
      let frame = makeRuntimeFrame(request.runtimeId, request.render_revision, rapidSurfaceTree());
      if (rapidMisrouteNextFrame) {
        rapidMisrouteNextFrame = false;
        // Corrupt only this response. The live sibling's own frame must remain
        // intact if it receives an independent resize while the test runs.
        frame = { ...frame, runtimeId: 'runtime-rapid-sibling',
          frameSequence: (frameSequenceByRuntime.get(request.runtimeId) ?? 0) + 10,
          tree: { type: 'Text', children: ['Wrong runtime frame'] } };
      }
      if (kind === 'input' && rapidNextAckExternalValue !== undefined) {
        const nextValue = rapidNextAckExternalValue;
        rapidNextAckExternalValue = undefined;
        const tree = frame.tree;
        let injected = false;
        Object.defineProperty(frame, 'tree', {
          enumerable: true,
          get() {
            if (!injected) {
              injected = true;
              queueMicrotask(() => {
                flushSync(() => emitRapidBackgroundFrame(nextValue));
                rapidAckRaceFired = true;
              });
            }
            return tree;
          },
        });
      }
      call.returnedRevision = frame.renderRevision;
      call.returnedFrameSequence = frame.frameSequence;
      call.returnedRuntimeId = frame.runtimeId;
      return frame as UiClientOperationResponseFor<T>;
    }
    if (session === rapidSessionId && request.runtimeId === 'runtime-rapid-input'
      && (request.type === 'resize' || request.type === 'render' || request.type === 'setProps')) {
      if (holdRapidRevisionFrame && request.render_revision === rapidRevision && request.type !== 'resize') {
        holdRapidRevisionFrame = false;
        await new Promise<void>((resolve) => { rapidRevisionFrameRelease = resolve; });
        rapidPropValue = rapidNextRevisionValue;
      }
      const frame = makeRuntimeFrame(request.runtimeId, request.render_revision, rapidSurfaceTree());
      call.returnedRevision = frame.renderRevision;
      call.returnedFrameSequence = frame.frameSequence;
      return frame as UiClientOperationResponseFor<T>;
    }
    const runtimeId = request.runtimeId;
    const tree = request.type === 'setProps' && frameIdentityByRuntime.get(runtimeId)?.client.key === 'status'
      ? surfaceTree(title)
      : runtimeFrames.get(runtimeId)?.tree ?? (frameIdentityByRuntime.get(runtimeId)?.client.key === 'status' ? surfaceTree() : { type: 'Text', children: ['Sibling Alive'] });
    const frame = makeRuntimeFrame(runtimeId, request.render_revision, tree);
    call.returnedRevision = frame.renderRevision;
    call.returnedFrameSequence = frame.frameSequence;
    if (request.type === 'resize' && holdInitialResize && runtimeId === currentStatusRuntimeId) {
      holdInitialResize = false;
      return await holdFrameResult(frame) as UiClientOperationResponseFor<T>;
    }
    if (request.type === 'setProps' && holdNextFrame) {
      holdNextFrame = false;
      return await holdFrameResult(frame) as UiClientOperationResponseFor<T>;
    }
    return frame as UiClientOperationResponseFor<T>;
  },
  onFrame(listener) { frameListeners.add(listener); return () => frameListeners.delete(listener); },
  onInvalidate(listener) { invalidationListeners.add(listener); return () => invalidationListeners.delete(listener); },
};

function Fixture() {
  const [enabled, updateEnabled] = React.useState(true);
  setEnabled = updateEnabled;
  const [lateEnabled, updateLateEnabled] = React.useState(false);
  setLateEnabled = updateLateEnabled;
  const [rapidEnabled, updateRapidEnabled] = React.useState(false);
  setRapidInputEnabled = updateRapidEnabled;
  const [identityClientsAttached, updateIdentityClientsAttached] = React.useState(false);
  React.useEffect(() => {
    const primaryLifecycle = new UiSurfaceLifecycleQueue();
    const probeLifecycle = new UiSurfaceLifecycleQueue();
    let active = true;
    void Promise.all([
      primaryLifecycle.attach(lifecycleHost, sessionId, primaryClientId),
      probeLifecycle.attach(lifecycleHost, sessionId, probeClientId),
    ]).then(() => { if (active) updateIdentityClientsAttached(true); });
    return () => {
      active = false;
      void primaryLifecycle.detach(lifecycleHost, sessionId, primaryClientId);
      void probeLifecycle.detach(lifecycleHost, sessionId, probeClientId);
    };
  }, []);
  return <Theme.Provider value={tokens(false)}>
    <main className="desktop-main" style={{ width: 720, height: 480, padding: 12 }}>
      {identityClientsAttached && enabled && <ModUiSurfaceClientIdProvider clientId={primaryClientId}><ModUiAbovePrompt
        sessionId={sessionId}
        host={host}
        engineFallback={({ ref, requestProps, responseProps }) => (
          <div
            data-engine-ref={ref}
            data-engine-request-props={Object.keys(requestProps).join(',')}
            className="fixture-engine-fallback"
          >{typeof responseProps.engineLabel === 'string' ? responseProps.engineLabel : 'Original engine row'}</div>
        )}
      /></ModUiSurfaceClientIdProvider>}
      {identityClientsAttached && <div style={{ display: 'none' }}><ModUiSurfaceClientIdProvider clientId={probeClientId}>
        <ModUiParentSite sessionId={sessionId} host={host} surface="desktop" component="InfoNotice"
          instanceId="identity-probe-client-two" props={identityProbeProps} viewport={identityProbeViewport} />
      </ModUiSurfaceClientIdProvider></div>}
      {rapidEnabled && <ModUiParentSite sessionId={rapidSessionId} host={host} surface="desktop" component="AbovePrompt"
        instanceId={rapidSiteInstanceId} props={{}} />}
      <div style={{ display: 'none' }}>{lateEnabled && <ModUiAbovePrompt sessionId={lateSessionId} host={host} />}</div>
      <BetaComposer bridge={composerBridge} ready={false} onOpenSettings={() => undefined} onOpenSettingsPage={() => undefined}
        onSetTheme={() => undefined} onOpenProviderSettings={() => undefined} />
    </main>
  </Theme.Provider>;
}

window.__modUiClientFixture = {
  calls,
  raceNextRapidAck(value) { rapidAckRaceFired = false; rapidNextAckExternalValue = value; },
  captureRapidInput() { capturedRapidInput = document.querySelector<HTMLInputElement>('[data-client="rapid-input"] .mod-ui-client-input'); },
  invalidateRapidSite(value) {
    rapidRevision += 1;
    rapidNextRevisionValue = value;
    holdRapidRevisionFrame = true;
    for (const listener of invalidationListeners) listener({
      sessionId: rapidSessionId,
      uuid: `rapid-revision-${rapidRevision}`,
      instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: rapidSiteInstanceId }],
    });
  },
  releaseRapidRevisionFrame() { rapidRevisionFrameRelease?.(); rapidRevisionFrameRelease = null; },
  enableRapidSibling() {
    rapidSiblingEnabled = true;
    rapidRevision += 1;
    for (const listener of invalidationListeners) listener({
      sessionId: rapidSessionId,
      uuid: `rapid-sibling-${rapidRevision}`,
      instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: rapidSiteInstanceId }],
    });
  },
  misrouteNextRapidFrame() { rapidMisrouteNextFrame = true; },
  enableRapidGroups() { rapidGroupsEnabled = true; rapidGroupsReversed = false; emitRapidBackgroundFrame(rapidPropValue); },
  swapRapidGroups() { rapidGroupsReversed = !rapidGroupsReversed; emitRapidBackgroundFrame(rapidPropValue); },
  holdNextRapidControl() { holdNextRapidControl = true; },
  holdNextRapidRunRejection() { holdNextRapidRunRejection = true; },
  releaseRapidRunRejection() { rapidRunRejectionRelease?.(); rapidRunRejectionRelease = null; },
  replaceRapidSite() {
    flushSync(() => setRapidInputEnabled(false));
    rapidSiteInstanceId = 'rapid-input-site-replacement';
    flushSync(() => setRapidInputEnabled(true));
  },
  releaseRapidControl() { rapidControlRelease?.(); rapidControlRelease = null; },
  startRapidInputScenario(mode = 'identity') { rapidInputMode = mode; setRapidInputEnabled(true); },
  setRapidInputMode(mode) { rapidInputMode = mode; },
  holdRapidPostAt(ordinal) { rapidHeldPostOrdinals.add(ordinal); },
  releaseRapidPostAt(ordinal) { rapidHeldPostReleases.get(ordinal)?.(); rapidHeldPostReleases.delete(ordinal); },
  ignoreNextRapidRunAsStale() { rapidIgnoreNextRunAsStale = true; },
  emitRapidExternalValue(value) {
    rapidPropValue = value;
    emitRapidBackgroundFrame(value);
  },
  rapidInputSnapshot() {
    const input = document.querySelector<HTMLInputElement>('[data-client="rapid-input"] .mod-ui-client-input');
    const prompt = document.querySelector<HTMLElement>('.beta-rich-prompt');
    return {
      value: input?.value ?? null,
      clientFocused: Boolean(input && document.activeElement === input),
      sameInputNode: input !== null && input === capturedRapidInput,
      inputReadOnly: input?.readOnly ?? null,
      unconfirmedNotice: input?.closest('label')?.querySelector('[role="status"]')?.textContent ?? null,
      retryLabel: input?.closest('form')?.querySelector('.mod-ui-client-input-retry')?.textContent ?? null,
      rapidRevision,
      revisionFrameHeld: rapidRevisionFrameRelease !== null,
      rapidControlHeld: rapidControlRelease !== null,
      rapidRunRejectionHeld: rapidRunRejectionRelease !== null,
      rapidRunRejections,
      rapidSiteInstanceId,
      promptText: prompt?.textContent ?? null,
      mainPromptSubmitCount,
      rapidControlOrdinal,
      rapidPostOrdinal,
      rapidAckRaceFired,
      rapidInputValue,
      rapidPropValue,
      rapidBackgroundPending: [...rapidBackgroundPending],
      rapidInputResult,
      rapidInputSubmitCount,
      rapidButtonSubmitCount,
      rapidInputCallbacks: [...rapidInputCallbacks],
      rapidOnInputCallbacks: [...rapidOnInputCallbacks],
      rapidOnSubmitCallbacks: [...rapidOnSubmitCallbacks],
      rapidButtonCallbacks: [...rapidButtonCallbacks],
      controlRequests: calls.filter((call) => call.kind === 'control' && call.sessionId === rapidSessionId
        && call.request.subtype === 'ui_client_press').map((call) => call.kind === 'control' ? call.request.event : null),
      runHeld: calls.filter((call) => call.kind === 'operation' && call.sessionId === rapidSessionId
        && call.request.type === 'runHeld').map((call) => call.kind === 'operation' && call.request.type === 'runHeld'
          ? { handle: call.request.handle, event: call.request.event, revision: call.request.render_revision } : null),
    };
  },
  contextMentionHref() { return fixtureContextMentionHref; },
  contextMentionEvents() { return [...contextMentionEvents]; },
  clientIdentitySnapshot() {
    const renderCalls = calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_render'
      && call.sessionId === sessionId);
    const primaryRender = renderCalls.find((call) => call.kind === 'control'
      && call.request.subtype === 'ui_render' && call.request.component === 'AbovePrompt');
    const probeRender = renderCalls.find((call) => call.kind === 'control'
      && call.request.subtype === 'ui_render' && call.request.instance_id === 'identity-probe-client-two');
    return {
      attachedClientIds: lifecycleCalls.filter((call) => call.sessionId === sessionId && call.command.type === 'ui_attach')
        .map((call) => call.command.type === 'ui_attach' ? call.command.client_id : ''),
      primaryRenderClientId: primaryRender?.kind === 'control' && primaryRender.request.subtype === 'ui_render'
        ? primaryRender.request.client_id : undefined,
      probeRenderClientId: probeRender?.kind === 'control' && probeRender.request.subtype === 'ui_render'
        ? probeRender.request.client_id : undefined,
      parentControlClientIds: calls.filter((call) => call.kind === 'control' && call.sessionId === sessionId
        && (call.request.subtype === 'ui_press' || call.request.subtype === 'ui_input' || call.request.subtype === 'ui_select'))
        .map((call) => call.kind === 'control' && (call.request.subtype === 'ui_press'
          || call.request.subtype === 'ui_input' || call.request.subtype === 'ui_select') ? call.request.client_id : undefined)
        .filter((clientId): clientId is string => clientId !== undefined),
    };
  },
  debugSnapshot() {
    return {
      currentStatusRuntimeId,
      statusEpochs: [...statusMountCountsByEpoch.entries()],
      initialStatusResizeHeld: !holdInitialResize && heldFrameOperationReleases.length > 0,
      heldFrameOperations: heldFrameOperationReleases.length,
      statusDom: [...document.querySelectorAll('[data-client="status"]')].map((element) => ({
        text: element.textContent,
        module: element.getAttribute('data-module'),
        fallbackVisible: element.querySelector('.mod-ui-client-fallback') !== null,
      })),
      recentCalls: calls.slice(-30).map((call) => ({
        kind: call.kind,
        sessionId: call.sessionId,
        request: call.request,
        returnedRuntimeId: call.returnedRuntimeId,
        returnedRevision: call.returnedRevision,
        returnedFrameSequence: call.returnedFrameSequence,
        workerFault: call.workerFault,
      })),
    };
  },
  updateParentProps(nextTitle) {
    title = nextTitle;
    revision += 1;
    const renderCall = latestAbovePromptRenderCall();
    if (!renderCall || renderCall.kind !== 'control' || renderCall.request.subtype !== 'ui_render') return;
    for (const listener of invalidationListeners) listener({
      sessionId,
      uuid: `invalidation-${revision}`,
      instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: renderCall.request.instance_id }],
    });
  },
  rerenderParentWithoutClientChange() {
    revision += 1;
    const renderCall = latestAbovePromptRenderCall();
    if (!renderCall || renderCall.kind !== 'control' || renderCall.request.subtype !== 'ui_render') return;
    for (const listener of invalidationListeners) listener({
      sessionId,
      uuid: `same-client-rerender-${revision}`,
      instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: renderCall.request.instance_id }],
    });
  },
  advanceClientStateToken() {
    clientStateVersion += 1;
    revision += 1;
    const renderCall = latestAbovePromptRenderCall();
    if (!renderCall || renderCall.kind !== 'control' || renderCall.request.subtype !== 'ui_render') return;
    for (const listener of invalidationListeners) listener({
      sessionId,
      uuid: `client-state-version-${revision}`,
      instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: renderCall.request.instance_id }],
    });
  },
  unloadStatusPlugin() {
    statusPluginLoaded = false;
    revision += 1;
    const renderCall = latestAbovePromptRenderCall();
    if (!renderCall || renderCall.kind !== 'control' || renderCall.request.subtype !== 'ui_render') return;
    for (const listener of invalidationListeners) listener({
      sessionId,
      uuid: `plugin-unloaded-${revision}`,
      instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: renderCall.request.instance_id }],
    });
  },
  loadStatusPlugin() {
    statusPluginLoaded = true;
    revision += 1;
    const renderCall = latestAbovePromptRenderCall();
    if (!renderCall || renderCall.kind !== 'control' || renderCall.request.subtype !== 'ui_render') return;
    for (const listener of invalidationListeners) listener({
      sessionId,
      uuid: `plugin-loaded-${revision}`,
      instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: renderCall.request.instance_id }],
    });
  },
  emitStaleFrame() {
    emitSnapshotFrame(sessionId, currentStatusRuntimeId, {
      renderRevision: revision - 1,
      frameSequence: 10_000,
      tree: { type: 'Text', children: ['Stale revision frame'] },
      hasPointerListener: false,
      hasKeyListener: false,
    });
    emitSnapshotFrame('old-session', currentStatusRuntimeId, {
      renderRevision: revision,
      frameSequence: 10_001,
      tree: { type: 'Text', children: ['Old session frame'] },
      hasPointerListener: false,
      hasKeyListener: false,
    });
  },
  rejectNextRunAtAdapter() { rejectRunAtAdapter = true; },
  ignoreNextRunAsStale() { ignoreRunAsStale = true; },
  showNoMods() {
    noParent = true;
    revision += 1;
    const renderCall = latestAbovePromptRenderCall();
    if (!renderCall || renderCall.kind !== 'control' || renderCall.request.subtype !== 'ui_render') return;
    for (const listener of invalidationListeners) listener({
      sessionId,
      uuid: `no-mods-${revision}`,
      instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: renderCall.request.instance_id }],
    });
  },
  disable() { setEnabled(false); },
  failNextRunInWorker() { failRunInWorker = true; },
  startLateRender() { setLateEnabled(true); },
  requestLateRerender() {
    const renderCall = [...calls].reverse().find((call) => call.kind === 'control' && call.sessionId === lateSessionId && call.request.subtype === 'ui_render');
    if (!renderCall || renderCall.kind !== 'control' || renderCall.request.subtype !== 'ui_render') return;
    const nextRevision = nextLateRevision;
    for (const listener of invalidationListeners) listener({
      sessionId: lateSessionId,
      uuid: `late-invalidation-${nextRevision}`,
      instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: renderCall.request.instance_id }],
    });
  },
  releaseLateRenderAt(index) { lateRenderResolvers.splice(index, 1)[0]?.resolve(); },
  disableLateRender() { setLateEnabled(false); },
  holdNextFrameOperation() { holdNextFrame = true; },
  releaseHeldFrameOperations() { for (const release of heldFrameOperationReleases.splice(0)) release(); },
  initialStatusResizeHeld() { return !holdInitialResize && heldFrameOperationReleases.length > 0; },
  initialMountsHeld() { return initialStatusMountRelease !== null && initialSiblingMountRelease !== null; },
  releaseInitialMounts() {
    initialStatusMountRelease?.();
    initialSiblingMountRelease?.();
    initialStatusMountRelease = null;
    initialSiblingMountRelease = null;
  },
  emitNewerAsyncFrame() {
    const setPropsCall = [...calls].reverse().find((call) => call.kind === 'operation' && call.request.type === 'setProps' && call.request.runtimeId === currentStatusRuntimeId);
    if (!setPropsCall || setPropsCall.kind !== 'operation' || setPropsCall.request.type !== 'setProps') return;
    emitRuntimeFrame(sessionId, makeRuntimeFrame(currentStatusRuntimeId, setPropsCall.request.render_revision, surfaceTree('Newest asynchronous frame')));
  },
  emitHighSequenceFrameForFailedRuntime(runtimeId) {
    if (!failedRuntimeIds.has(runtimeId)) return;
    emitSnapshotFrame(sessionId, runtimeId, {
      renderRevision: revision,
      frameSequence: 50_000,
      tree: surfaceTree('Late failed runtime frame'),
      hasPointerListener: false,
      hasKeyListener: false,
    });
  },
  reloadStatusPluginEnvironment() {
    clientRuntimeEpochs = { ...clientRuntimeEpochs, 'fixture-plugin': (clientRuntimeEpochs['fixture-plugin'] ?? 0) + 1 };
    revision += 1;
    const renderCall = latestAbovePromptRenderCall();
    if (!renderCall || renderCall.kind !== 'control' || renderCall.request.subtype !== 'ui_render') return;
    for (const listener of invalidationListeners) listener({
      sessionId,
      uuid: `environment-reload-${clientRuntimeEpochs['fixture-plugin']}`,
      instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: renderCall.request.instance_id }],
    });
  },
  emitAsyncWorkerFault() {
    failedRuntimeIds.add(currentStatusRuntimeId);
    emitWorkerFaultFrame(sessionId, currentStatusRuntimeId, revision, 'fixture asynchronous worker failed');
  },
  enableAsyncFaultClient() {
    asyncFaultClientEnabled = true;
    revision += 1;
    const renderCall = latestAbovePromptRenderCall();
    if (!renderCall || renderCall.kind !== 'control' || renderCall.request.subtype !== 'ui_render') return;
    for (const listener of invalidationListeners) listener({
      sessionId,
      uuid: `async-client-mounted-${revision}`,
      instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: renderCall.request.instance_id }],
    });
  },
  disableAsyncFaultClient() {
    asyncFaultClientEnabled = false;
    revision += 1;
    const renderCall = latestAbovePromptRenderCall();
    if (!renderCall || renderCall.kind !== 'control' || renderCall.request.subtype !== 'ui_render') return;
    for (const listener of invalidationListeners) listener({
      sessionId,
      uuid: `async-client-unmounted-${revision}`,
      instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: renderCall.request.instance_id }],
    });
  },
};

createRoot(document.getElementById('root')!).render(<Fixture />);
