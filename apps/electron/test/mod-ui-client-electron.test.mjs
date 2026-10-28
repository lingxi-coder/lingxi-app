import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';

import react from '@vitejs/plugin-react';
import { build } from 'vite';

const electronRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const fixtureRoot = join(electronRoot, 'test', 'fixtures');
const electronBinary = resolve(electronRoot, 'node_modules/electron/cli.js');
const electronDriver = join(fixtureRoot, 'mod-ui-client-electron.mjs');

test('Electron renders Native Client input and Select held events while keeping Markdown links ordinary', { timeout: 90_000 }, async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-mod-ui-client-vite-'));
  const fixtureBuildDir = mkdtempSync(join(tmpdir(), 'lingxi-mod-ui-client-build-'));
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-mod-ui-client-electron-'));
  const fixtureHtml = join(fixtureRoot, 'mod-ui-client-fixture.html');
  let child;
  try {
    await build({
      root: fixtureRoot,
      base: './',
      cacheDir: viteCacheDir,
      configFile: false,
      logLevel: 'error',
      plugins: [{
        name: 'mod-ui-negative-controls',
        enforce: 'pre',
        transform(source, id) {
          if (id.endsWith('/modUiClientTree.tsx')) {
            let result = source;
            if (process.env.LINGXI_TEST_DISABLE_ANCESTOR_KEYS === '1') {
            const keyed = `function childKey(node: ModUiSurfaceNode, index: number): string {
  const key = explicitNodeKey(node);
  return key === null ? \`index:\${node.type}:\${index}\` : \`key:\${key}\`;
}`;
            assert.ok(result.includes(keyed), 'negative control must remove keyed ancestor reconciliation');
            result = result.replace(keyed, `function childKey(node: ModUiSurfaceNode, index: number): string {
  if (node.type === 'Button' || node.type === 'Input' || node.type === 'Select') return \`\${node.type}:\${node.props.key}\`;
  if (node.type === 'Markdown' && node.props.key) return \`Markdown:\${node.props.key}\`;
  return \`\${node.type}:\${index}\`;
}`);
            }
            if (process.env.LINGXI_TEST_DISABLE_STALE_RUN_REJECTION_FENCE === '1') {
              const guarded = `    return queued.catch((error: unknown) => {
      if (lifecycle.current !== activeGeneration || eventEpoch !== dispatchEpochRef.current || !dispatchReadyRef.current) return undefined;`;
              assert.ok(result.includes(guarded), 'negative control must remove the stale dispatch fault fence');
              result = result.replace(guarded, `    return queued.catch((error: unknown) => {
      if (lifecycle.current !== activeGeneration) return undefined;`);
            }
            if (process.env.LINGXI_TEST_ALLOW_UNCONFIRMED_ENTER === '1') {
              const guard = `    if (unconfirmedDraftRef.current || submitPendingRef.current) return;
    void dispatchInput('submit', event.currentTarget.value);`;
              assert.ok(result.includes(guard), 'negative control must allow Enter to submit an unconfirmed draft');
              result = result.replace(guard, `    if (submitPendingRef.current) return;
    void dispatchInput('submit', event.currentTarget.value);`);
            }
            if (process.env.LINGXI_TEST_ALLOW_DUPLICATE_RETRY === '1') {
              const guard = 'if (!kind || disabled || !frameCurrent || retryPendingRef.current || submitPendingRef.current) return;';
              assert.ok(result.includes(guard), 'negative control must allow a duplicate retry');
              result = result.replace(guard, 'if (!kind || disabled || !frameCurrent) return;');
              const submitGuard = '      if (submitPendingRef.current) return Promise.resolve();';
              assert.ok(result.includes(submitGuard), 'negative control must allow a duplicate submit');
              result = result.replace(submitGuard, '');
            }
            if (process.env.LINGXI_TEST_DISABLE_RETRY_FOCUS === '1') {
              const restore = `          if (unconfirmedDraftRef.current && document.activeElement === retryButtonRef.current) {
            inputElementRef.current?.focus({ preventScroll: true });
          }
`;
              assert.ok(result.includes(restore), 'negative control must remove retry focus restoration');
              result = result.replace(restore, '');
            }
            if (process.env.LINGXI_TEST_CLEAR_UNCONFIRMED_SUBMIT_ON_CHANGE === '1') {
              const guard = "if (unconfirmedDraftRef.current !== 'submit' || edit.kind === 'submit') {";
              assert.ok(result.includes(guard), 'negative control must let a change confirm an uncertain submit');
              result = result.replace(guard, 'if (true) {');
            }
            if (process.env.LINGXI_TEST_RETRY_CURRENT_DRAFT === '1') {
              const value = "const retryValue = kind === 'submit' ? unconfirmedSubmitValueRef.current : draft;";
              assert.ok(result.includes(value), 'negative control must retry the new draft instead of the original submit');
              result = result.replace(value, 'const retryValue = draft;');
            }
            if (process.env.LINGXI_TEST_RETRY_WRONG_KIND === 'change' || process.env.LINGXI_TEST_RETRY_WRONG_KIND === 'submit') {
              const retry = 'void dispatchInput(kind, retryValue, true).finally(() => {';
              assert.ok(result.includes(retry), 'negative control must change the retry event kind');
              result = result.replace(retry, `void dispatchInput('${process.env.LINGXI_TEST_RETRY_WRONG_KIND}', retryValue, true).finally(() => {`);
            }
            return result === source ? null : result;
          }
          if (!id.endsWith('/modUiAbovePrompt.tsx')) return null;
          let result = source;
          if (process.env.LINGXI_TEST_RESOLVE_HANDLE_EARLY === '1') {
            const queuedHandle = `    const run = operationTail.current.catch(() => undefined).then(() => {
      const operation = buildOperation();
      return operation ? context.host.operation(context.site.sessionId, operation) : null;
    });`;
            assert.ok(result.includes(queuedHandle), 'negative control must resolve the held handle before the queue');
            result = result.replace(queuedHandle, `    const operation = buildOperation();
    const run = operationTail.current.catch(() => undefined).then(() =>
      operation ? context.host.operation(context.site.sessionId, operation) : null);`);
          }
          if (process.env.LINGXI_TEST_DROP_NOOP_WARNING === '1') {
            const guard = 'if (frameResult === null || isStaleOperation(frameResult)) return superseded;';
            assert.ok(result.includes(guard), 'negative control must discard an uncertain noop');
            result = result.replace(guard, 'if (frameResult === null || isStaleOperation(frameResult)) return;');
          }
          if (process.env.LINGXI_TEST_THROW_UNKNOWN_INPUT === '1') {
            const guard = "if (request.event.type === 'input') return nativeInputSuperseded;";
            assert.ok(result.includes(guard), 'negative control must lose the uncertain input on an operation error');
            result = result.replace(guard, "if (request.event.type === 'input') throw error;");
          }
          if (process.env.LINGXI_TEST_DISABLE_QUEUED_OWNER_FENCE === '1') {
            const guard = `      if (runtimeIdRef.current !== runtimeAtStart || currentRevisionRef.current !== revision
        || operationGeneration.current !== generation || !context.isRevisionCurrent(revision)
        || failedRuntimeEpochRef.current === runtimeEpochRef.current || failedRuntimeIdRef.current === runtimeAtStart) return null;`;
            assert.ok(result.includes(guard), 'negative control must remove queued owner checks');
            result = result.replace(guard, '');
          }
          const parentCommit = `      setParentFrame({
        sessionId,
        revision,
        response: result.response,
        clientRuntimeEpochs: result.metadata?.clientRuntimeEpochs ?? {},
        clientStateToken: result.metadata?.clientStateToken ?? null,
      });`;
          assert.ok(result.includes(parentCommit), 'fixture must be able to hold the parent commit');
          result = result.replace(parentCommit, `      const commitParentFrame = () => setParentFrame({
        sessionId,
        revision,
        response: result.response,
        clientRuntimeEpochs: result.metadata?.clientRuntimeEpochs ?? {},
        clientStateToken: result.metadata?.clientStateToken ?? null,
      });
      if ((window as any).__holdRapidParentCommit && sessionId === 'mod-ui-client-rapid-input-session') {
        (window as any).__heldRapidParentCommit = commitParentFrame;
      } else commitParentFrame();`);
          if (process.env.LINGXI_TEST_DISABLE_STALE_RUN_REJECTION_FENCE === '1') {
            const guardedFault = `    if (!context || currentRevisionRef.current !== revision || context.site.committedRevision !== revision
      || !context.isRevisionCurrent(revision)) return;`;
            assert.ok(result.includes(guardedFault), 'negative control must remove Host revision fault validation');
            result = result.replace(guardedFault, `    if (!context || currentRevisionRef.current !== revision || context.site.committedRevision !== revision) return;`);
            const guardedRun = `    let frameResult;
    try {
      frameResult = await queueHeldOperation(buildOperation);
    } catch (error) {
      if (runtimeIdRef.current !== runtimeAtStart || currentRevisionRef.current !== revision
        || operationGeneration.current !== generation || !context.isRevisionCurrent(revision)) return superseded;
      // A transport or protocol failure can arrive after the callback ran.
      // Preserve the Input and require an explicit decision before retrying.
      if (request.event.type === 'input') return nativeInputSuperseded;
      throw error;
    }`;
            assert.ok(result.includes(guardedRun), 'negative control must remove stale run rejection validation');
            result = result.replace(guardedRun, '    const frameResult = await queueHeldOperation(buildOperation);');
          }
          if (process.env.LINGXI_TEST_DISABLE_SUPERSEDED_MARKER === '1') {
            const marker = "const superseded = request.event.type === 'input' ? nativeInputSuperseded : undefined;";
            assert.ok(result.includes(marker), 'negative control must remove the superseded input marker');
            result = result.replace(marker, 'const superseded = undefined;');
          }
          if (process.env.LINGXI_TEST_DELAY_PASSIVE_CLEANUP === '1') {
            const cleanup = `    return () => {
      active = false;
      if (generation === operationGeneration.current) operationGeneration.current += 1;
    };`;
            assert.ok(result.includes(cleanup), 'fixture must delay the operation passive cleanup');
            result = result.replace(cleanup, `    return () => {
      const dispose = () => {
        active = false;
        if (generation === operationGeneration.current) operationGeneration.current += 1;
      };
      if ((window as any).__deferRapidPassiveCleanup) window.setTimeout(dispose, 500);
      else dispose();
    };`);
          }
          if (process.env.LINGXI_TEST_DISABLE_OWNER_FENCE === '1') {
            const ownerFence = `  useLayoutEffect(() => () => {
    // Invalidate in-flight control calls at commit, before passive cleanup can
    // run or a replacement instance can mount a runtime with the same id.
    operationGeneration.current += 1;
    const currentRuntimeId = runtimeIdRef.current;
    const revision = mountedRevisionRef.current;
    if (!currentRuntimeId || revision === null) return;
    runtimeIdRef.current = null;
    unmountOperationRef.current(currentRuntimeId, revision);
  }, [descriptor.key, descriptor.module, descriptor.plugin, site?.instanceId, site?.sessionId]);`;
            assert.ok(result.includes(ownerFence), 'negative control must restore passive owner cleanup');
            result = result.replace(ownerFence, `  useEffect(() => () => {
    const dispose = () => {
      const currentRuntimeId = runtimeIdRef.current;
      const revision = mountedRevisionRef.current;
      if (!currentRuntimeId || revision === null) return;
      runtimeIdRef.current = null;
      unmountOperationRef.current(currentRuntimeId, revision);
    };
    if ((window as any).__deferRapidPassiveCleanup) window.setTimeout(dispose, 500);
    else dispose();
  }, [descriptor.key, descriptor.module, descriptor.plugin, site?.instanceId, site?.sessionId]);`);
          }
          if (process.env.LINGXI_TEST_RESTORE_REVISION_GATE === '1') {
            const retainedFrame = ': frame ? <ModUiClientTree';
            assert.ok(result.includes(retainedFrame), 'negative control must restore the old revision gate');
            result = result.replace(retainedFrame, ': frame && frame.renderRevision === site?.renderRevision ? <ModUiClientTree');
          }
          if (process.env.LINGXI_TEST_DISABLE_RUNTIME_CHECK === '1') {
            const runtimeCheck = `    if (isFrame(frameResult) && frameResult.runtimeId !== runtimeAtStart) {
      throw new Error('Client module host returned a frame for a different runtime');
    }
`;
            assert.ok(result.includes(runtimeCheck), 'negative control must remove the runtime identity check');
            result = result.replace(runtimeCheck, '');
          }
          if (process.env.LINGXI_TEST_REJECT_OLD_SUCCESS_ACK === '1') {
            const oldFrame = `      if (!acceptFrame(frameResult, revision, runtimeAtStart)) {
        const latest = frameRef.current;
        // A later frame may arrive on the independent event channel while the
        // successful runHeld response is in transit. Do not roll that frame
        // back, but do acknowledge the completed callback. This relies on the
        // Host contract that rejected runHeld handles return handled:false,
        // and that a frame is returned only after acceptance and settlement
        // of the callback's synchronous posts.
        if (!latest || latest.renderRevision !== revision || latest.frameSequence < frameResult.frameSequence
          || failedRuntimeEpochRef.current === runtimeEpochRef.current || failedRuntimeIdRef.current === runtimeAtStart) return;
      }`;
            assert.ok(result.includes(oldFrame), 'negative control must discard the older successful acknowledgement');
            result = result.replace(oldFrame, '      if (!acceptFrame(frameResult, revision, runtimeAtStart)) return;');
          }
          if (process.env.LINGXI_TEST_DISABLE_ACK_FRESHNESS !== '1') return result === source ? null : result;
          const current = `return createNativeInputAck(acknowledgedValue, () => {
        if (runtimeIdRef.current !== runtimeAtStart || operationGeneration.current !== generation
          || currentRevisionRef.current !== revision || !context.isRevisionCurrent(revision)) return nativeInputSuperseded;
        const latest = frameRef.current;
        if (latest?.renderRevision === revision && latest.frameSequence > frameResult.frameSequence) {
          return inputValueForElement(latest.tree, request.element, descriptor.plugin);
        }
        return acknowledgedValue;
      });`;
          assert.ok(result.includes(current), 'negative control must remove the Host acknowledgement freshness check');
          return result.replace(current, 'return createNativeInputAck(acknowledgedValue);');
        },
      }, react()],
      resolve: {
        alias: { '@renderer': resolve(electronRoot, 'src/renderer') },
        dedupe: ['react', 'react-dom'],
      },
      build: { outDir: fixtureBuildDir, emptyOutDir: true, rollupOptions: { input: fixtureHtml } },
    });
    const fixtureUrl = pathToFileURL(join(fixtureBuildDir, 'mod-ui-client-fixture.html')).href;
    const output = [];
    const errors = [];
    child = spawn(process.execPath, [electronBinary, electronDriver, fixtureUrl], {
      cwd: electronRoot,
      env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: temporaryUserData },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => output.push(String(chunk)));
    child.stderr.on('data', (chunk) => errors.push(String(chunk)));
    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron Mod UI Client fixture timed out\n${errors.join('')}`));
      }, 75_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) {
          rejectResult(new Error(`Electron Mod UI Client fixture exited ${code ?? signal}\n${errors.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid Electron Mod UI Client fixture output: ${line}`, { cause: error })); }
      });
    });

    assert.equal(result.initialStatusMountSuperseded, true);
    assert.equal(result.initialSiblingMountSuperseded, true);
    assert.equal(result.parentElementsRendered, true);
    assert.equal(result.parentControlRequestsMatchNativeBodies, true, JSON.stringify({
      press: result.parentButtonRequest,
      inputChange: result.parentInputChangeRequest,
      inputSubmit: result.parentInputSubmitRequest,
      select: result.parentSelectRequest,
      markdown: result.parentMarkdownRequest,
    }));
    assert.equal(result.clientIdentityPropagationMatchesAttach, true, JSON.stringify(result.clientIdentitySnapshot));
    assert.equal(result.buttonPressHandled, true);
    assert.equal(result.buttonUsesCurrentHandle, true);
    assert.equal(result.inputUsesHookReachedValue, true);
    assert.equal(result.inputSubmitUsesHookReachedValue, true);
    assert.deepEqual(result.inputRunHeldEvents, [
      { kind: 'change', value: 'normalized-input' },
      { kind: 'submit', value: 'normalized-submitted-input' },
    ]);
    assert.equal(result.selectUsesHookReachedValue, true);
    assert.deepEqual(result.selectRunHeldEvents, [{ value: 'week' }]);
    assert.equal(result.markdownExternalLinkUsesNormalNavigation, true);
    assert.equal(result.markdownOpenLinkUsesNormalNavigation, true);
    assert.equal(result.markdownLinksDoNotDispatchClientCallbacks, true);
    assert.equal(result.contextMentionClickPreventedNavigation, true);
    assert.deepEqual(result.contextMentionOpenEvents, [{ kind: 'plugin', name: 'Fixture', target: 'plugin:fixture' }]);
    assert.equal(result.ordinaryMentionClickPreventedNavigation, true);
    assert.deepEqual(result.ordinaryMentionOpenEvents, [
      { kind: 'plugin', name: 'Fixture', target: 'plugin:fixture' },
      { kind: 'plugin', name: 'Fixture', target: 'plugin:fixture' },
    ]);
    assert.equal(result.preRegistrationFrameReplayed, true);
    assert.equal(result.staleOperationIgnoredWithoutFault, true);
    assert.equal(result.parentPropsUpdateRendered, true);
    assert.equal(result.staleRpcFrameIgnored, true);
    assert.equal(result.staleFrameIgnored, true);
    assert.equal(result.workerFaultRenderedWithoutDuplicateReport, true, JSON.stringify(result.workerFaultDiagnostics));
    assert.equal(result.workerFaultLeavesSiblingVisible, true);
    assert.equal(result.workerFaultNotClearedByParentRevision, true);
    assert.equal(result.workerFaultRecoveredFromEnvironmentRemount, true);
    assert.equal(result.asyncWorkerFaultRenderedWithoutDuplicateReport, true);
    assert.equal(result.bufferedAsyncWorkerFaultIsLeafLocal, true);
    assert.equal(result.statusReadyAfterAsyncClientRemoval, true, JSON.stringify(result.statusReadinessTimeline));
    assert.equal(result.adapterFaultReportedAsRun, true);
    assert.equal(result.faultLeavesSiblingVisible, true);
    assert.equal(result.adapterRunFaultSurvivedPassiveFrames, true);
    assert.equal(result.adapterRunFaultSurvivedNewRevisionSameIdentity, true);
    assert.equal(result.adapterRunFaultSurvivedSameIdentityPropsUpdate, true);
    assert.equal(result.adapterRunFaultRecoveredAfterStateTokenChange, true);
    assert.equal(result.adapterRunFaultRecoveredAfterClientRemount, true);
    assert.equal(result.pluginUnloadInvalidationRendered, true);
    assert.equal(result.emptyNoHostIsQuietAndCleansUp, true);
    assert.equal(result.runtimeLifecycleSnapshot.pluginEnvironmentRemountsAllClients, true, JSON.stringify(result.runtimeLifecycleSnapshot));
    assert.equal(result.unmountReleasedRuntimeAndDrawIdentity, true, JSON.stringify(result.runtimeLifecycleSnapshot));
    assert.equal(result.staleRenderCleanupPreservedNewerFrame, true);
    assert.equal(result.lateInitialRenderCleanedUp, true);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    rmSync(viteCacheDir, { recursive: true, force: true });
    rmSync(fixtureBuildDir, { recursive: true, force: true });
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
