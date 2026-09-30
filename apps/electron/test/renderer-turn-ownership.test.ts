import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';
import * as tracked from '../src/renderer/bridge/trackedTurns';
import { matchesComposerSubmission } from '../src/renderer/bridge/composerSubmission';
import { VoiceFlowController } from '../src/renderer/audio/flow/controller';
import { audioConfigurationDefaults } from '../src/shared/generatedAudioConfiguration';
import { emptyConversation, appendPendingUserPrompt, reduceEvent } from '../src/renderer/bridge/conversation';

function source(path: string) {
  return ts.createSourceFile(path, readFileSync(resolve(path), 'utf8'), ts.ScriptTarget.Latest, true,
    path.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
}
const bridgeSource = source('src/renderer/bridge/useBridge.ts');
const composerSource = source('src/renderer/components/BetaDesktop.tsx');
function find(file: ts.SourceFile, predicate: (node: ts.Node) => boolean): any {
  let found: ts.Node | undefined;
  const walk = (node: ts.Node): void => { if (!found && predicate(node)) found = node; ts.forEachChild(node, walk); };
  walk(file); assert.ok(found, file.fileName); return found;
}
function declaration(file: ts.SourceFile, name: string): string {
  return find(file, (node) => ts.isVariableDeclaration(node) && node.name.getText(file) === name).initializer.getText(file);
}
function evaluate(code: string, context: any): void {
  runInNewContext(ts.transpileModule(code, { compilerOptions: {
    target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS,
  } }).outputText, context);
}
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: Error) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
const flush = () => new Promise((resolve) => setImmediate(resolve));
function eventHarness(purpose: 'flow' | 'composer' = 'flow') {
  const context: any = { ...tracked, useCallback: (fn: any) => fn,
    activeTrackedTurns: { current: new Map() }, pendingTrackedTurns: { current: new Map() }, trackedSpeechListeners: { current: new Map() },
    removedRuntimeIds: { current: new Set() }, bootstrapRef: { current: null }, pendingSessionRef: { current: null }, sharedDesktopCacheRef: { current: new Map() },
    turnActiveRefs: { current: new Map() }, engineTurnActiveRefs: { current: new Map() }, slashPendingRefs: { current: new Map() }, pendingFusionDispatches: { current: new Map() }, sideQuestionTurns: { current: new Map() },
    cancellingRefs: { current: new Map() }, cancellationTasks: { current: new Map() }, activeSessionIdRef: { current: 'A' },
    cancelledTrackedTokens: { current: new Set() }, pendingTrackedDispatches: { current: new Set() },
    updateRuntime: () => {}, scheduleProjectCatalogRefresh: () => {}, clearSlashTurnClaim: () => {}, isPendingFusionSessionRestore: () => false,
    shouldReleaseSlashTurn: () => false, host: { command: async () => {} },
  };
  evaluate('globalThis.completeTrackedSpeech = ' + declaration(bridgeSource, 'completeTrackedSpeech'), context);
  const call = find(bridgeSource, (node) => ts.isCallExpression(node) && node.expression.getText(bridgeSource) === 'host.onEvent');
  evaluate('globalThis.onEvent = ' + call.arguments[0].getText(bridgeSource), context);
  const token = tracked.createDesktopTurnToken('A', 1, purpose, 123);
  tracked.enqueueTrackedTurn(context.pendingTrackedTurns.current, token);
  const speech: any[] = [];
  context.trackedSpeechListeners.current.set(token.clientTurnId, new Set([(event: any) => speech.push(event)]));
  return { context, token, speech, emit: (event: any) => context.onEvent({ sessionId: 'A', sequence: 1, event }) };
}

test('message boundaries preserve prompt ownership through tool rounds and only turn_ended completes speech', () => {
  const h = eventHarness();
  h.emit({ type: 'turn_started', turn_id: 123 });
  h.emit({ type: 'text_delta', text: 'Inspecting the file. ' });
  h.emit({ type: 'message_identity', message_id: 'first' });
  h.emit({ type: 'message_complete', stop_reason: 'tool_use' });
  assert.equal(h.speech.filter((event) => event.type === 'completion').length, 0);
  assert.equal(h.context.activeTrackedTurns.current.size, 1);
  h.emit({ type: 'tool_use_started', id: 'tool', tool: 'Read' });
  h.emit({ type: 'text_delta', text: 'The final answer.' });
  h.emit({ type: 'message_identity', message_id: 'last' });
  h.emit({ type: 'message_complete', stop_reason: 'end_turn' });
  assert.equal(h.speech.filter((event) => event.type === 'completion').length, 0);
  h.emit({ type: 'turn_ended', outcome: { type: 'end_turn' } });
  const completed = h.speech.filter((event) => event.type === 'completion');
  assert.equal(completed.length, 1);
  assert.equal(completed[0].text, 'Inspecting the file. The final answer.');
  assert.equal(completed[0].terminal, 'turn_ended');
  assert.deepEqual(h.speech.filter((event) => event.type === 'message').map((event) => event.text),
    ['Inspecting the file. ', 'The final answer.']);
});

test('SDK boundary-before-retraction retry never publishes rejected text for playback', () => {
  const h = eventHarness(); h.emit({ type: 'turn_started', turn_id: 123 });
  h.emit({ type: 'text_delta', text: 'Rejected malformed invoke.' });
  h.emit({ type: 'message_identity', message_id: 'rejected' });
  h.emit({ type: 'message_complete', stop_reason: 'tool_use' });
  h.emit({ type: 'message_retracted', message_id: 'rejected' });
  h.emit({ type: 'text_delta', text: 'The accepted answer.' });
  h.emit({ type: 'message_identity', message_id: 'accepted' });
  h.emit({ type: 'message_complete', stop_reason: 'end_turn' });
  h.emit({ type: 'turn_ended', outcome: { type: 'end_turn' } });
  assert.deepEqual(h.speech.filter((event) => event.type === 'message').map((event) => event.text), ['The accepted answer.']);
  assert.equal(h.speech.find((event) => event.type === 'completion').text, 'The accepted answer.');
});

test('Flow waits for the turn terminal before relistening and speaks accepted message segments once', async () => {
  const h = eventHarness(), operations: any[] = [], timers: Array<() => void> = [], pendingListen = deferred<any>();
  let listening = 0;
  const controller = new VoiceFlowController({
    bridge: { sendTrackedPrompt: () => ({ token: h.token, queued: Promise.resolve() }),
      subscribeTrackedSpeech: (token, listener) => { h.context.trackedSpeechListeners.current.set(token.clientTurnId, new Set([listener])); return () => h.context.trackedSpeechListeners.current.delete(token.clientTurnId); },
      cancelTrackedPrompt: async () => {} },
    audio: { execute: async (operation) => { operations.push(operation); return { result: operation.type === 'listen'
      ? ++listening === 1 ? { type: 'transcript', text: 'Inspect and fix this file' } : await pendingListen.promise
      : { type: 'playback_completed' } }; }, cancel: async () => {}, finishListen: async () => {} },
    timers: { setTimeout: (callback) => { timers.push(callback); return callback; }, clearTimeout: () => {} },
    getPreferences: () => ({ configuration: audioConfigurationDefaults(), revision: 0 }), onStateChange: () => {},
  });
  await controller.start(); h.emit({ type: 'turn_started', turn_id: 123 });
  h.emit({ type: 'text_delta', text: 'Rejected draft.' }); h.emit({ type: 'message_identity', message_id: 'retry' });
  h.emit({ type: 'message_complete', stop_reason: 'tool_use' }); h.emit({ type: 'message_retracted', message_id: 'retry' });
  h.emit({ type: 'text_delta', text: 'Inspecting the file.' }); h.emit({ type: 'message_identity', message_id: 'first' });
  h.emit({ type: 'message_complete', stop_reason: 'tool_use' }); await flush();
  assert.equal(operations.filter((operation) => operation.type === 'speak').length, 0);
  assert.equal(timers.length, 0);
  h.emit({ type: 'tool_use_started', id: 'read', tool: 'Read' }); await flush();
  h.emit({ type: 'text_delta', text: 'The file is fixed.' }); h.emit({ type: 'message_complete', stop_reason: 'end_turn' }); await flush();
  assert.equal(timers.length, 0); assert.equal(listening, 1);
  h.emit({ type: 'turn_ended', outcome: { type: 'end_turn' } }); await flush();
  assert.deepEqual(operations.filter((operation) => operation.type === 'speak').map((operation) => operation.text), ['Inspecting the file.', 'The file is fixed.']);
  assert.equal(timers.length, 1); timers.shift()!(); await flush(); assert.equal(listening, 2);
  controller.dispose(); pendingListen.resolve({ type: 'failed', error: { kind: 'cancelled', message: 'test teardown' } });
});

test('admission binding ignores unrelated engine turns and ID allocation survives independent hook lifetimes', () => {
  const pending = new Map(), active = new Map();
  const first = tracked.createDesktopTurnToken('A', 1, 'flow', tracked.allocateTrackedTurnId());
  const second = tracked.createDesktopTurnToken('A', 1, 'flow', tracked.allocateTrackedTurnId());
  assert.notEqual(first.turnId, second.turnId); assert.ok(Number.isSafeInteger(first.turnId));
  tracked.enqueueTrackedTurn(pending, first); tracked.enqueueTrackedTurn(pending, second);
  assert.equal(tracked.bindTrackedTurn(pending, active, 'A', { type: 'turn_started' }), null);
  assert.equal(tracked.bindTrackedTurn(pending, active, 'A', { type: 'turn_started', turn_id: 7 }), null);
  assert.equal(pending.get('A').length, 2);
  assert.equal(tracked.bindTrackedTurn(pending, active, 'A', { type: 'turn_started', turn_id: second.turnId })?.token, second);
  assert.equal(pending.get('A')[0], first);
});

function composerHarness() {
  const queued = deferred<void>(), notices: unknown[] = [], revoked: string[] = [];
  let images: any[] = [];
  const editor = { innerHTML: 'First prompt', replaceChildren() { this.innerHTML = ''; } };
  const context: any = { ready: true, flowModeRef: { current: false }, voiceState: 'idle', text: 'First prompt', selectedFiles: [], imageAttachments: images,
    draftSessionId: { current: 'A' }, draftsBySession: { current: new Map() }, imageDraftGeneration: { current: 1 }, composerDraftRevision: { current: 0 }, imageAttachmentsRef: { current: images },
    input: { current: editor }, richPromptSnapshot: () => ({ text: editor.innerHTML, files: [] }), promptWithFileMentions: (text: string) => text,
    bridge: { running: false, desktop: { slashCommands: [] }, sendTrackedPrompt: () => ({ token: { sessionId: 'A', clientTurnId: 'A:1' }, queued: queued.promise }) },
    autoplayOwner: { current: null }, autoplaySubscription: { current: null },
    voicePrefs: { autoPlayReplies: false }, audio: undefined, URL: { revokeObjectURL: (url: string) => revoked.push(url) }, slashDismissed: { current: false },
    setText: () => {}, setSelectedFiles: () => {}, setImageAttachments: (update: any) => { images = update(images); context.imageAttachmentsRef.current = images; },
    setImageNotice: (notice: unknown) => notices.push(notice), setFilePicker: () => {}, setSlashQuery: () => {}, matchesComposerSubmission,
  };
  for (const name of ['savedEditorSelection', 'activeMentionRange', 'activeSlashRange', 'activeSlashQuery']) context[name] = { current: null };
  evaluate('globalThis.clearComposer = ' + declaration(composerSource, 'clearComposer'), context);
  evaluate('globalThis.submit = ' + declaration(composerSource, 'submit'), context);
  return { queued, context, editor, notices, revoked, images: () => images,
    changeImages(next: any[]) { images = next; context.imageAttachmentsRef.current = next; } };
}

function autoplayComposerHarness() {
  const composer = composerHarness(), events = eventHarness('composer'), cleanup = deferred<void>();
  const spoken: any[] = [];
  Object.assign(composer.context, {
    audio: {}, voicePrefs: { ...audioConfigurationDefaults(), autoPlayReplies: true },
    activeAudioSessionId: { current: 'A' }, document: { hidden: false },
    sanitizeSpeakableText: (text: string) => text, resolveAudioLanguage: () => 'en-US',
    shouldAutoplayTrackedReply: (active: string, owner: string, hidden: boolean) => active === owner && !hidden,
    cancelAutoplay: () => cleanup.promise,
  });
  Object.assign(composer.context.bridge, {
    bootstrap: { settings: { voiceRevision: 0 } },
    sendTrackedPrompt: () => ({ token: events.token, queued: composer.queued.promise }),
    subscribeTrackedSpeech: (token: any, callback: any) => {
      const listeners = events.context.trackedSpeechListeners.current.get(token.clientTurnId) ?? new Set();
      listeners.add(callback); events.context.trackedSpeechListeners.current.set(token.clientTurnId, listeners);
      return () => listeners.delete(callback);
    },
    audioExecute: async (operation: any) => { spoken.push(operation); return { result: { type: 'playback_completed' } }; },
  });
  const finishFastTurn = () => {
    events.emit({ type: 'turn_started', turn_id: 123 });
    events.emit({ type: 'text_delta', text: 'Fast accepted answer.' });
    events.emit({ type: 'message_identity', message_id: 'fast-answer' });
    events.emit({ type: 'message_complete', stop_reason: 'end_turn' });
    events.emit({ type: 'turn_ended', outcome: { type: 'end_turn' } });
  };
  return { ...composer, events, cleanup, spoken, finishFastTurn };
}

test('composer captures fast terminal before old audio cleanup finishes and plays it once afterward', async () => {
  const h = autoplayComposerHarness(), submission = h.context.submit();
  assert.equal(h.events.context.trackedSpeechListeners.current.get(h.events.token.clientTurnId).size, 2);
  h.finishFastTurn();
  assert.equal(h.spoken.length, 0);
  assert.equal(h.context.autoplaySubscription.current, null);
  assert.equal(h.events.context.trackedSpeechListeners.current.has(h.events.token.clientTurnId), false);
  h.cleanup.resolve(); await flush();
  assert.deepEqual(h.spoken.map((operation) => operation.text), ['Fast accepted answer.']);
  assert.equal(h.context.autoplayOwner.current, null);
  h.queued.resolve(); await submission;
});

test('late audio cleanup cannot play a navigated reply or clear a replacement autoplay owner', async () => {
  const h = autoplayComposerHarness(), submission = h.context.submit();
  h.finishFastTurn();
  h.context.activeAudioSessionId.current = 'B';
  h.context.draftSessionId.current = 'B';
  h.context.autoplayOwner.current = { kind: 'autoplay', id: 'B:replacement' };
  h.cleanup.resolve(); await flush();
  assert.equal(h.spoken.length, 0);
  assert.equal(h.context.autoplayOwner.current.id, 'B:replacement');
  h.queued.resolve(); await submission;
});

test('failed prompt dispatch removes its autoplay subscription while old cleanup is pending', async () => {
  const h = autoplayComposerHarness(), submission = h.context.submit();
  const listeners = h.events.context.trackedSpeechListeners.current.get(h.events.token.clientTurnId);
  assert.equal(listeners.size, 2);
  h.queued.reject(new Error('fake dispatch rejection')); await submission;
  assert.equal(listeners.size, 1);
  assert.equal(h.context.autoplaySubscription.current, null);
  assert.equal(h.context.autoplayOwner.current, null);
  h.cleanup.resolve(); await flush();
  assert.equal(h.spoken.length, 0);
});

test('a delayed dispatch ACK preserves later text and attachment edits', async () => {
  const h = composerHarness(), sent = h.context.submit();
  h.editor.innerHTML = 'The next draft'; h.changeImages([{ id: 'new', previewUrl: 'blob:new' }]);
  h.queued.resolve(); await sent;
  assert.equal(h.editor.innerHTML, 'The next draft'); assert.equal(h.images()[0].id, 'new'); assert.deepEqual(h.revoked, []);
});

test('unchanged submitted draft clears, while navigation and failed sends retain the original owner', async () => {
  const unchanged = composerHarness(), sent = unchanged.context.submit(); unchanged.queued.resolve(); await sent;
  assert.equal(unchanged.editor.innerHTML, '');
  const moved = composerHarness(), failed = moved.context.submit();
  moved.context.draftSessionId.current = 'B'; moved.context.imageDraftGeneration.current += 1; moved.editor.innerHTML = 'Private B';
  moved.queued.reject(new Error('A dispatch failed')); await failed;
  assert.equal(moved.editor.innerHTML, 'Private B'); assert.deepEqual(moved.notices, []);
});

test('Flow stop and interruption cancel their captured prompt before any ID-bearing engine event', async () => {
  const cancelled: any[] = [], sent: any[] = []; let sequence = 0;
  const makeController = () => new VoiceFlowController({
    bridge: { sendTrackedPrompt: (text) => { const token = { sessionId: 'A', clientTurnId: `A:${++sequence}`, purpose: 'flow' as const, turnId: sequence }; sent.push({ text, token }); return { token, queued: Promise.resolve() }; },
      subscribeTrackedSpeech: () => () => {}, cancelTrackedPrompt: async (token) => { cancelled.push(token); } },
    audio: { execute: async () => ({ result: { type: 'transcript', text: sent.length ? 'Replacement question' : 'Original question' } }), cancel: async () => {}, finishListen: async () => {} },
    timers: { setTimeout: () => 0, clearTimeout: () => {} }, getPreferences: () => ({ configuration: audioConfigurationDefaults(), revision: 0 }), onStateChange: () => {},
  });
  const stopped = makeController(); await stopped.start(); await stopped.stop(); assert.equal(cancelled[0], sent[0].token);
  const interrupted = makeController(); await interrupted.start(); const original = sent.at(-1).token;
  await interrupted.orb(); assert.equal(cancelled.at(-1), original); assert.notEqual(sent.at(-1).token, original);
  interrupted.dispose(); assert.equal(cancelled.at(-1), sent.at(-1).token);
});

test('owned cancellation targets A after navigation to B and a settled token cannot cancel newer work', async () => {
  const h = eventHarness(), calls: unknown[] = [];
  h.context.host.cancel = async (sessionId: string, turnId: number) => { calls.push([sessionId, turnId]); };
  h.context.activeSessionIdRef.current = 'B'; h.context.turnActiveRefs.current.set('B', true);
  evaluate('globalThis.cancelTrackedPrompt = ' + declaration(bridgeSource, 'cancelTrackedPrompt'), h.context);
  await h.context.cancelTrackedPrompt(h.token);
  assert.deepEqual(calls, [['A', 123]]); assert.equal(h.context.turnActiveRefs.current.get('B'), true);
  await h.context.cancelTrackedPrompt(h.token); assert.equal(calls.length, 1);
});

test('an old cancelled hydration failure cannot release a replacement turn or put its error on session B', async () => {
  const h = eventHarness(), old = deferred<void>(), newer = deferred<void>(), calls: any[] = [], errors: unknown[] = [];
  h.context.pendingTrackedTurns.current.clear(); let sequence = 0, state: any = { conversation: emptyConversation(), runtimeCenter: {} };
  Object.assign(h.context, {
    sessionLoadingRef: { current: false }, runtimeResourceSendSequence: { current: 0 }, trackedTurnSequence: { current: 0 },
    promptRuntimeResources: () => [], submittedSessionFor: () => undefined, appendPendingUserPrompt, addRuntimeResources: (value: unknown) => value,
    acknowledgePromptDispatch: (value: unknown) => value, commitRuntimeResources: (value: unknown) => value, rollbackRuntimeResources: (value: unknown) => value,
    reduceEvent, reduceEventWithPendingFusion: reduceEvent, reduceDesktopEvent: (value: unknown) => value, reduceRuntimeCenterEvent: (value: unknown) => value, capture: (error: unknown) => { errors.push(error); throw error; },
    updateRuntime: (_id: string, update: any) => { state = update(state); },
  });
  h.context.host.sendPrompt = async (...args: any[]) => { calls.push(args); return ++sequence === 1 ? old.promise : newer.promise; };
  h.context.host.cancel = async () => {};
  evaluate('globalThis.sendTrackedPrompt = ' + declaration(bridgeSource, 'sendTrackedPrompt'), h.context);
  evaluate('globalThis.cancelTrackedPrompt = ' + declaration(bridgeSource, 'cancelTrackedPrompt'), h.context);
  const first = h.context.sendTrackedPrompt('Original question'); const rejected = assert.rejects(first.queued, /interrupted/);
  await h.context.cancelTrackedPrompt(first.token);
  const next = h.context.sendTrackedPrompt('Replacement question');
  h.emit({ type: 'turn_started', turn_id: next.token.turnId });
  h.context.activeSessionIdRef.current = 'B'; old.reject(new Error('interrupted')); await rejected;
  assert.equal(h.context.turnActiveRefs.current.get('A'), true); assert.equal(state.conversation.running, true);
  assert.deepEqual(errors, []); assert.equal(calls[0][3], first.token.turnId); assert.notEqual(calls[1][3], calls[0][3]);
  newer.resolve(); await next.queued;
});

test('an image selected during dispatch can finish reading after the ACK without being invalidated', async () => {
  const h = composerHarness(), read = deferred<any>();
  Object.assign(h.context, { activeSessionId: 'A', MAX_IMAGE_ATTACHMENTS: 4, imageFileToAttachment: () => read.promise });
  evaluate('globalThis.addImageFiles = ' + declaration(composerSource, 'addImageFiles'), h.context);
  const sent = h.context.submit(); const attached = h.context.addImageFiles([{ name: 'next.png' }]);
  h.queued.resolve(); await sent;
  assert.equal(h.editor.innerHTML, 'First prompt');
  read.resolve({ id: 'next', name: 'next.png', previewUrl: 'blob:next' }); await attached;
  assert.equal(h.images()[0].id, 'next'); assert.deepEqual(h.revoked, []);
});

test('slow native teardown does not postpone cancellation of pending prompt admission', async () => {
  const audioCancel = deferred<void>(), calls: any[] = [], token = tracked.createDesktopTurnToken('A', 1, 'flow', 123);
  const controller = new VoiceFlowController({
    bridge: { sendTrackedPrompt: () => ({ token, queued: Promise.resolve() }), subscribeTrackedSpeech: () => () => {},
      cancelTrackedPrompt: async (owner) => { calls.push(owner); } },
    audio: { execute: async () => ({ result: { type: 'transcript', text: 'Original question' } }),
      cancel: () => audioCancel.promise, finishListen: async () => {} },
    timers: { setTimeout: () => 0, clearTimeout: () => {} }, getPreferences: () => ({ configuration: audioConfigurationDefaults(), revision: 0 }), onStateChange: () => {},
  });
  await controller.start(); const stopped = controller.stop();
  assert.equal(calls[0], token); audioCancel.resolve(); await stopped;
});

test('failed owned cancellation does not submit a competing replacement Flow prompt', async () => {
  const sent: string[] = [], token = tracked.createDesktopTurnToken('A', 1, 'flow', 123);
  const controller = new VoiceFlowController({
    bridge: { sendTrackedPrompt: (text) => { sent.push(text); return { token, queued: Promise.resolve() }; },
      subscribeTrackedSpeech: () => () => {}, cancelTrackedPrompt: async () => { throw new Error('transport unavailable'); } },
    audio: { execute: async () => ({ result: { type: 'transcript', text: sent.length ? 'Replacement' : 'Original' } }), cancel: async () => {}, finishListen: async () => {} },
    timers: { setTimeout: () => 0, clearTimeout: () => {} }, getPreferences: () => ({ configuration: audioConfigurationDefaults(), revision: 0 }), onStateChange: () => {},
  });
  await controller.start(); await controller.orb();
  assert.deepEqual(sent, ['Original']); assert.equal(controller.getState().phase, 'failed');
  controller.dispose();
});
