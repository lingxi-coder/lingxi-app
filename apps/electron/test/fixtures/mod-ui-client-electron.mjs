import { app, BrowserWindow } from 'electron';

const url = process.argv.find((argument) => argument.startsWith('http://') || argument.startsWith('file://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 900, height: 600, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const run = (script) => window.webContents.executeJavaScript(script);
    const wait = async (expression) => {
      const deadline = Date.now() + 8_000;
      while (Date.now() < deadline) {
        if (await run(expression)) return;
        await delay(25);
      }
      throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
    };
    await wait(`Boolean(document.querySelector('.mod-ui-client-button')) && document.querySelector('[data-client="sibling"]')?.textContent.includes('Sibling Alive')`);

    const parentElementsRendered = await run(`Boolean(document.querySelector('.mod-ui-parent-button'))
      && Boolean(document.querySelector('.mod-ui-parent-input input'))
      && Boolean(document.querySelector('.mod-ui-parent-select-label select'))
      && Boolean(document.querySelector('.mod-ui-parent-link[href="https://example.com/parent"]'))
      && Boolean(document.querySelector('.mod-ui-parent-code[data-path="parent.ts"][data-start-line="4"]'))
      && Boolean(document.querySelector('.mod-ui-parent-markdown'))
      && Boolean(document.querySelector('.mod-ui-parent-svg[alt="Parent diagram"]'))
      && document.querySelector('.fixture-engine-fallback')?.textContent === 'Response engine row'
      && document.querySelector('.fixture-engine-fallback')?.getAttribute('data-engine-ref') === '0'`);

    await run(`document.querySelector('.mod-ui-parent-button').click()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'control' && call.request.subtype === 'ui_press' && call.request.key === 'parent-run')`);
    const parentButtonRequest = await run(`(() => window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_press' && call.request.key === 'parent-run').at(-1)?.request)()`);

    await run(`(() => {
      const input = document.querySelector('.mod-ui-parent-input input');
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'parent-typed');
      input.dispatchEvent(new Event('input', { bubbles: true }));
    })()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'control' && call.request.subtype === 'ui_input' && call.request.kind === 'change' && call.request.value === 'parent-typed')`);
    const parentInputChangeRequest = await run(`(() => window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_input' && call.request.kind === 'change').at(-1)?.request)()`);
    await run(`document.querySelector('.mod-ui-parent-input').dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }))`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'control' && call.request.subtype === 'ui_input' && call.request.kind === 'submit' && call.request.value === 'parent-typed')`);
    const parentInputSubmitRequest = await run(`(() => window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_input' && call.request.kind === 'submit').at(-1)?.request)()`);

    await run(`(() => {
      const select = document.querySelector('.mod-ui-parent-select-label select');
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(select, 'week');
      select.dispatchEvent(new Event('change', { bubbles: true }));
    })()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'control' && call.request.subtype === 'ui_select' && call.request.key === 'parent-period' && call.request.value === 'week')`);
    const parentSelectRequest = await run(`(() => window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_select').at(-1)?.request)()`);

    await run(`document.querySelector('.mod-ui-parent-markdown a[href]')?.click()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'control' && call.request.subtype === 'ui_press' && call.request.key === 'parent-markdown')`);
    const parentMarkdownRequest = await run(`(() => window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_press' && call.request.key === 'parent-markdown').at(-1)?.request)()`);
    const parentControlRequestsMatchNativeBodies = await run(`(() => {
      const calls = window.__modUiClientFixture.calls;
      const render = calls.find((call) => call.kind === 'control' && call.request.subtype === 'ui_render' && call.request.component === 'AbovePrompt');
      const button = calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_press' && call.request.key === 'parent-run').at(-1)?.request;
      const inputChange = calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_input' && call.request.kind === 'change').at(-1)?.request;
      const inputSubmit = calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_input' && call.request.kind === 'submit').at(-1)?.request;
      const select = calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_select').at(-1)?.request;
      const markdown = calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_press' && call.request.key === 'parent-markdown').at(-1)?.request;
      return render?.request.component === 'AbovePrompt'
        && button.key === 'parent-run'
        && button.plugin === 'fixture-plugin'
        && button.handle === 31
        && button.surface === 'desktop'
        && button.component === undefined
        && button.instance_id === undefined
        && inputChange.component === render.request.component
        && inputChange.instance_id === render.request.instance_id
        && inputChange.kind === 'change'
        && inputChange.value === 'parent-typed'
        && inputSubmit.kind === 'submit'
        && inputSubmit.value === 'parent-typed'
        && select.component === render.request.component
        && select.instance_id === render.request.instance_id
        && select.value === 'week'
        && markdown.key === 'parent-markdown'
        && markdown.href === 'https://example.com/docs';
    })()`);
    const clientIdentitySnapshot = await run(`window.__modUiClientFixture.clientIdentitySnapshot()`);
    const attachedClientIds = new Set(clientIdentitySnapshot.attachedClientIds);
    const clientIdentityPropagationMatchesAttach = attachedClientIds.size === 2
      && clientIdentitySnapshot.primaryRenderClientId === 'fixture-window-client-one'
      && clientIdentitySnapshot.probeRenderClientId === 'fixture-window-client-two'
      && attachedClientIds.has(clientIdentitySnapshot.primaryRenderClientId)
      && attachedClientIds.has(clientIdentitySnapshot.probeRenderClientId)
      && clientIdentitySnapshot.parentControlClientIds.length >= 5
      && clientIdentitySnapshot.parentControlClientIds.every((clientId) => clientId === clientIdentitySnapshot.primaryRenderClientId);

    const drawBeforeMount = await run(`(() => {
      const calls = window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation');
      const commit = calls.findIndex((call) => call.request.type === 'draw_commit' && call.domCommitted);
      const mount = calls.findIndex((call) => call.request.type === 'mount');
      return commit >= 0 && mount > commit;
    })()`);
    if (!drawBeforeMount) throw new Error('Client module mounted before its parent draw commit observed committed DOM');
    await wait(`document.querySelector('.mod-ui-client-surface')?.textContent.includes('Scheduled final frame')`);
    const preRegistrationFrameReplayed = await run(`document.querySelector('.mod-ui-client-surface')?.textContent.includes('Scheduled final frame') && !document.querySelector('.mod-ui-client-surface')?.textContent.includes('Mount response frame') && !document.querySelector('.mod-ui-client-surface')?.textContent.includes('Stale buffered frame')`);
    await run(`window.__modUiClientFixture.releaseHeldFrameOperations()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'resize' && call.request.runtimeId === 'runtime-status')`);

    await run(`document.querySelector('.mod-ui-client-button').click()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 17)`);
    const buttonPressHandled = await run(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_client_press' && call.request.event.type === 'press').length === 1`);
    const buttonUsesCurrentHandle = await run(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 17 && !('event' in call.request)).length === 1`);

    await run(`(() => {
      const input = document.querySelector('.mod-ui-client-input');
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'typed');
      input.dispatchEvent(new Event('input', { bubbles: true }));
    })()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 18 && call.request.event === 'normalized-input')`);
    const inputUsesHookReachedValue = await run(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 18 && call.request.event === 'normalized-input').length === 1`);

    await run(`(() => {
      const select = document.querySelector('.mod-ui-client-select');
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(select, 'week');
      select.dispatchEvent(new Event('change', { bubbles: true }));
    })()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 19 && call.request.event === 'week')`);
    const selectUsesHookReachedValue = await run(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 19 && call.request.event === 'week').length === 1`);

    await run(`window.__modUiClientFixture.ignoreNextRunAsStale(); document.querySelector('.mod-ui-client-button').click()`);
    await wait(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 17).length === 2`);
    await delay(80);
    const staleOperationIgnoredWithoutFault = await run(`!document.querySelector('.mod-ui-client-fallback') && window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_client_fault').length === 0`);

    await run(`window.__modUiClientFixture.holdNextFrameOperation(); window.__modUiClientFixture.updateParentProps('Updated from props')`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'setProps' && call.request.props.title === 'Updated from props')`);
    await run(`window.__modUiClientFixture.emitNewerAsyncFrame()`);
    await wait(`document.querySelector('.mod-ui-client-surface')?.textContent.includes('Newest asynchronous frame')`);
    await run(`window.__modUiClientFixture.releaseHeldFrameOperations()`);
    await delay(80);
    const parentPropsUpdateRendered = await run(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'setProps' && call.request.props.title === 'Updated from props')`);
    const staleRpcFrameIgnored = await run(`document.querySelector('.mod-ui-client-surface')?.textContent.includes('Newest asynchronous frame') && !document.querySelector('.mod-ui-client-surface')?.textContent.includes('Updated from props')`);

    await run(`window.__modUiClientFixture.emitStaleFrame()`);
    await delay(80);
    const staleFrameIgnored = await run(`!document.querySelector('.mod-ui-client-surface')?.textContent.includes('Stale revision frame') && !document.querySelector('.mod-ui-client-surface')?.textContent.includes('Old session frame')`);

    await run(`window.__modUiClientFixture.failNextRunInWorker(); document.querySelector('.mod-ui-client-button').click()`);
    await wait(`document.querySelector('.mod-ui-client-fallback') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.workerFault === true && call.request.type === 'runHeld' && call.request.handle === 17 && call.request.runtimeId === 'runtime-status')`);
    await run(`window.__modUiClientFixture.emitHighSequenceFrameForFailedRuntime('runtime-status')`);
    await delay(100);
    const workerFaultDiagnostics = await run(`(() => {
      const calls = window.__modUiClientFixture.calls;
      const faultIndex = calls.findIndex((call) => call.kind === 'operation' && call.workerFault === true);
      const postFaultRuntimeOperations = faultIndex < 0 ? [] : calls.slice(faultIndex + 1).filter((call) => call.kind === 'operation'
        && ['render', 'setProps', 'resize', 'pointer', 'key', 'runHeld'].includes(call.request.type));
      const duplicateFaultReports = calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_client_fault');
      return {
        fallbackVisible: Boolean(document.querySelector('.mod-ui-client-fallback')),
        faultIndex,
        staleHighSequenceTreeVisible: document.querySelector('.mod-ui-client-surface')?.textContent.includes('Late failed runtime frame') ?? false,
        postFaultRuntimeOperationTypes: postFaultRuntimeOperations.map((call) => call.request.type),
        duplicateFaultReportCount: duplicateFaultReports.length,
      };
    })()`);
    const workerFaultRenderedWithoutDuplicateReport = workerFaultDiagnostics.fallbackVisible
      && workerFaultDiagnostics.faultIndex >= 0
      && !workerFaultDiagnostics.staleHighSequenceTreeVisible
      && workerFaultDiagnostics.postFaultRuntimeOperationTypes.length === 0
      && workerFaultDiagnostics.duplicateFaultReportCount === 0;
    const workerFaultLeavesSiblingVisible = await run(`(() => {
      const calls = window.__modUiClientFixture.calls;
      const siblingMounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'mount' && call.request.client === 'sibling');
      const siblingUnmounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === 'runtime-sibling-1');
      return Boolean(document.querySelector('[data-client="sibling"]'))
        && document.querySelector('[data-client="sibling"]').textContent.includes('Sibling Alive')
        && siblingMounts.length === 1
        && siblingMounts[0].returnedRuntimeId === 'runtime-sibling-1'
        && siblingMounts[0].returnedClientEnvironmentEpoch === 1
        && siblingUnmounts.length === 0;
    })()`);

    const statusMountCountBeforeOrdinaryRevision = await run(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation' && call.request.type === 'mount' && call.request.client === 'status').length`);
    await run(`window.__modUiClientFixture.updateParentProps('Ordinary revision with failed environment')`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'control' && call.sessionId === 'mod-ui-client-fixture-session' && call.request.subtype === 'ui_render' && call.request.component === 'AbovePrompt' && call.returnedRevision === 3)`);
    await delay(80);
    const workerFaultNotClearedByParentRevision = await run(`(() => {
      const calls = window.__modUiClientFixture.calls;
      const faultIndex = calls.findIndex((call) => call.kind === 'operation' && call.workerFault === true);
      const sameRuntimeCalls = faultIndex < 0 ? [] : calls.slice(faultIndex + 1).filter((call) => call.kind === 'operation'
        && call.request.runtimeId === 'runtime-status'
        && ['render', 'setProps', 'resize', 'pointer', 'key', 'runHeld'].includes(call.request.type));
      const statusMounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'mount' && call.request.client === 'status');
      return document.querySelector('.mod-ui-client-fallback') !== null
        && statusMounts.length === ${statusMountCountBeforeOrdinaryRevision}
        && sameRuntimeCalls.length === 0;
    })()`);

    await run(`window.__modUiClientFixture.reloadStatusPluginEnvironment()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'mount' && call.returnedRuntimeId === 'runtime-status-epoch-2')`);
    await wait(`document.querySelector('.mod-ui-client-surface')?.textContent.includes('Recovered after environment remount')`);
    const workerFaultRecoveredFromEnvironmentRemount = await run(`!document.querySelector('.mod-ui-client-fallback') && document.querySelector('.mod-ui-client-surface')?.textContent.includes('Recovered after environment remount') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'mount' && call.returnedRuntimeId === 'runtime-status-epoch-2')`);

    await run(`window.__modUiClientFixture.emitAsyncWorkerFault()`);
    await wait(`document.querySelector('.mod-ui-client-fallback') !== null`);
    await run(`window.__modUiClientFixture.emitHighSequenceFrameForFailedRuntime('runtime-status-epoch-2')`);
    await delay(80);
    const asyncWorkerFaultRenderedWithoutDuplicateReport = await run(`(() => {
      const calls = window.__modUiClientFixture.calls;
      const siblingMounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'mount' && call.request.client === 'sibling');
      const currentSiblingUnmounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === 'runtime-sibling-2');
      return document.querySelector('.mod-ui-client-fallback') !== null
        && document.querySelector('[data-client="sibling"]')?.textContent.includes('Sibling Alive')
        && siblingMounts.length === 2
        && siblingMounts[1].returnedRuntimeId === 'runtime-sibling-2'
        && siblingMounts[1].returnedClientEnvironmentEpoch === 2
        && currentSiblingUnmounts.length === 0
        && calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_client_fault').length === 0;
    })()`);

    await run(`window.__modUiClientFixture.reloadStatusPluginEnvironment()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'mount' && call.returnedRuntimeId === 'runtime-status-epoch-3')`);
    await wait(`document.querySelector('.mod-ui-client-surface')?.textContent.includes('Recovered after environment remount')`);

    await run(`window.__modUiClientFixture.enableAsyncFaultClient()`);
    await wait(`document.querySelector('[data-client="async-fault"] .mod-ui-client-fallback') !== null`);
    const bufferedAsyncWorkerFaultIsLeafLocal = await run(`document.querySelector('[data-client="async-fault"] .mod-ui-client-fallback') !== null
      && document.querySelector('[data-client="status"] .mod-ui-client-fallback') === null
      && document.querySelector('[data-client="sibling"]')?.textContent.includes('Sibling Alive')
      && !document.querySelector('[data-client="async-fault"] .mod-ui-client-surface')?.textContent.includes('Buffered pre-fault frame')
      && window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_client_fault').length === 0`);
    await run(`window.__modUiClientFixture.disableAsyncFaultClient()`);
    await wait(`!document.querySelector('[data-client="async-fault"]') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === 'runtime-async-fault')`);
    const statusReadyAfterAsyncClientRemovalExpression = `(() => {
      const calls = window.__modUiClientFixture.calls;
      const latestRender = [...calls].reverse().find((call) => call.kind === 'control'
        && call.sessionId === 'mod-ui-client-fixture-session'
        && call.request.subtype === 'ui_render'
        && call.request.component === 'AbovePrompt'
        && call.returnedRevision !== undefined);
      return latestRender !== undefined && calls.some((call) => call.kind === 'operation'
        && call.request.type === 'render'
        && call.request.runtimeId === 'runtime-status-epoch-3'
        && call.request.render_revision === latestRender.returnedRevision);
    })()`;
    const statusReadinessTimelineExpression = `(() => {
      const calls = window.__modUiClientFixture.calls;
      const latestRender = [...calls].reverse().find((call) => call.kind === 'control'
        && call.sessionId === 'mod-ui-client-fixture-session'
        && call.request.subtype === 'ui_render'
        && call.request.component === 'AbovePrompt'
        && call.returnedRevision !== undefined);
      if (!latestRender) return [];
      return calls.filter((call) => (call.kind === 'control'
          && call.sessionId === 'mod-ui-client-fixture-session'
          && call.request.subtype === 'ui_render'
          && call.request.component === 'AbovePrompt'
          && call.returnedRevision === latestRender.returnedRevision)
        || (call.kind === 'operation'
          && (call.request.type === 'draw_commit' || (call.request.type === 'render' && call.request.runtimeId === 'runtime-status-epoch-3'))
          && call.request.render_revision === latestRender.returnedRevision))
        .map((call) => ({ kind: call.kind, type: call.kind === 'control' ? call.request.subtype : call.request.type,
          requestedRevision: call.kind === 'control' ? call.returnedRevision : call.request.render_revision,
          returnedRevision: call.returnedRevision, runtimeId: call.kind === 'operation' ? call.request.runtimeId : undefined,
          handled: call.handled, frameSequence: call.returnedFrameSequence }));
    })()`;
    try {
      await wait(statusReadyAfterAsyncClientRemovalExpression);
    } catch (error) {
      const timeline = await run(statusReadinessTimelineExpression);
      throw new Error(`Status Client did not become ready after async Client removal. ${error instanceof Error ? error.message : String(error)}\nStatus call timeline: ${JSON.stringify(timeline)}`, { cause: error });
    }
    const statusReadyAfterAsyncClientRemoval = await run(statusReadyAfterAsyncClientRemovalExpression);
    const statusReadinessTimeline = await run(statusReadinessTimelineExpression);

    await run(`window.__modUiClientFixture.rejectNextRunAtAdapter(); document.querySelector('.mod-ui-client-button').click()`);
    try {
      await wait(`document.querySelector('.mod-ui-client-fallback') && window.__modUiClientFixture.calls.some((call) => call.kind === 'control' && call.request.subtype === 'ui_client_fault' && call.request.phase === 'run')`);
    } catch (error) {
      const snapshot = await run(`window.__modUiClientFixture.debugSnapshot()`);
      throw new Error(`${error instanceof Error ? error.message : String(error)}\nClient identity/fault snapshot: ${JSON.stringify(snapshot)}`, { cause: error });
    }
    const adapterFaultReportedAsRun = await run(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_client_fault' && call.request.phase === 'run' && call.request.client === 'status').length === 1`);
    const faultLeavesSiblingVisible = await run(`Boolean(document.querySelector('[data-client="sibling"]')) && document.querySelector('[data-client="sibling"]').textContent.includes('Sibling Alive')`);
    const adapterRunFaultSurvivedPassiveFrames = await run(`document.querySelector('[data-client="status"] .mod-ui-client-fallback') !== null`);
    await run(`window.__modUiClientFixture.rerenderParentWithoutClientChange()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'control' && call.sessionId === 'mod-ui-client-fixture-session' && call.request.subtype === 'ui_render' && call.request.component === 'AbovePrompt' && call.returnedRevision === 8)`);
    const adapterRunFaultSurvivedNewRevisionSameIdentity = await run(`document.querySelector('[data-client="status"] .mod-ui-client-fallback') !== null`);
    await run(`window.__modUiClientFixture.updateParentProps('Changed props after run fault')`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'control' && call.sessionId === 'mod-ui-client-fixture-session' && call.request.subtype === 'ui_render' && call.request.component === 'AbovePrompt' && call.returnedRevision === 9)`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'setProps' && call.request.props.title === 'Changed props after run fault')`);
    const adapterRunFaultSurvivedSameIdentityPropsUpdate = await run(`document.querySelector('[data-client="status"] .mod-ui-client-fallback') !== null`);

    const statusRuntimeBeforeStateChange = await run(`window.__modUiClientFixture.debugSnapshot().currentStatusRuntimeId`);
    await run(`window.__modUiClientFixture.advanceClientStateToken()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'control' && call.sessionId === 'mod-ui-client-fixture-session' && call.request.subtype === 'ui_render' && call.request.component === 'AbovePrompt' && call.returnedRevision === 10)`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'render' && call.request.runtimeId === '${statusRuntimeBeforeStateChange}' && call.request.render_revision === 10)`);
    await wait(`document.querySelector('[data-client="status"] .mod-ui-client-fallback') === null && document.querySelector('[data-client="status"] .mod-ui-client-surface') !== null`);
    const adapterRunFaultRecoveredAfterStateTokenChange = await run(`document.querySelector('[data-client="status"] .mod-ui-client-fallback') === null && window.__modUiClientFixture.debugSnapshot().currentStatusRuntimeId === '${statusRuntimeBeforeStateChange}' && !window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'mount' && call.request.client === 'status' && call.returnedRevision === 10)`);

    await run(`window.__modUiClientFixture.rejectNextRunAtAdapter(); document.querySelector('[data-client="status"] .mod-ui-client-button').click()`);
    await wait(`document.querySelector('[data-client="status"] .mod-ui-client-fallback') !== null && window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_client_fault' && call.request.phase === 'run' && call.request.client === 'status').length === 2`);

    await run(`window.__modUiClientFixture.unloadStatusPlugin()`);
    await wait(`!document.querySelector('.mod-ui-client-instance[data-client="status"]') && document.querySelector('[data-client="sibling"]')?.textContent.includes('Sibling Alive') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === 'runtime-status-epoch-3')`);
    await run(`window.__modUiClientFixture.loadStatusPlugin()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'mount' && call.returnedRuntimeId === 'runtime-status-epoch-3-mount-2')`);
    await wait(`document.querySelector('[data-client="status"] .mod-ui-client-surface')?.textContent.includes('Recovered after environment remount')`);
    const adapterRunFaultRecoveredAfterClientRemount = await run(`document.querySelector('[data-client="status"] .mod-ui-client-fallback') === null && document.querySelector('[data-client="status"] .mod-ui-client-surface')?.textContent.includes('Recovered after environment remount')`);

    await run(`window.__modUiClientFixture.unloadStatusPlugin()`);
    await wait(`!document.querySelector('.mod-ui-client-instance[data-client="status"]') && document.querySelector('[data-client="sibling"]')?.textContent.includes('Sibling Alive') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === 'runtime-status-epoch-3-mount-2')`);
    const pluginUnloadInvalidationRendered = await run(`!document.querySelector('.mod-ui-client-instance[data-client="status"]') && Boolean(document.querySelector('[data-client="sibling"]')) && document.querySelector('[data-client="sibling"]').textContent.includes('Sibling Alive') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === 'runtime-status-epoch-3-mount-2')`);

    await run(`window.__modUiClientFixture.showNoMods()`);
    await wait(`!document.querySelector('.mod-ui-client-instance') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'draw_unmount')`);
    const emptyNoHostIsQuietAndCleansUp = await run(`!document.querySelector('.mod-ui-client-fallback') && !document.querySelector('.mod-ui-client-instance') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'draw_unmount')`);

    await run(`window.__modUiClientFixture.disable()`);
    await wait(`!document.querySelector('.mod-ui-above-prompt')`);
    await delay(80);
    const runtimeLifecycleSnapshot = await run(`(() => {
      const calls = window.__modUiClientFixture.calls.filter((call) => call.sessionId === 'mod-ui-client-fixture-session');
      const mounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'mount').map((call) => call.returnedRuntimeId);
      const unmounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'unmount').map((call) => call.request.runtimeId);
      const expected = ['runtime-status', 'runtime-status-epoch-2', 'runtime-status-epoch-3', 'runtime-status-epoch-3-mount-2', 'runtime-async-fault', 'runtime-sibling-1', 'runtime-sibling-2', 'runtime-sibling-3'];
      const counts = (items) => items.reduce((result, value) => {
        result[value] = (result[value] ?? 0) + 1;
        return result;
      }, {});
      const mountCounts = counts(mounts);
      const unmountCounts = counts(unmounts);
      const siblingMounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'mount' && call.request.client === 'sibling');
      const drawUnmounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'draw_unmount')
        .map((call) => ({ component: call.request.component, instanceId: call.request.instance_id,
          revision: call.request.render_revision, handled: call.handled }));
      const primaryDrawUnmounts = drawUnmounts.filter((call) => call.component === 'AbovePrompt');
      return {
        expectedRuntimeIds: expected,
        mountedRuntimeIds: mounts,
        unmountedRuntimeIds: unmounts,
        mountCounts,
        unmountCounts,
        missingExpectedMounts: expected.filter((runtimeId) => mountCounts[runtimeId] !== 1),
        unexpectedMountIds: mounts.filter((runtimeId) => !expected.includes(runtimeId)),
        missingOrRepeatedUnmounts: expected.filter((runtimeId) => unmountCounts[runtimeId] !== 1),
        unmountsWithoutMount: unmounts.filter((runtimeId) => mountCounts[runtimeId] !== 1),
        siblingMountsByPluginEpoch: siblingMounts.map((call) => ({ plugin: call.request.plugin, runtimeId: call.returnedRuntimeId, epoch: call.returnedClientEnvironmentEpoch })),
        pluginEnvironmentRemountsAllClients: siblingMounts.length === 3
          && siblingMounts.every((call, index) => call.request.plugin === 'fixture-plugin'
            && call.returnedRuntimeId === 'runtime-sibling-' + (index + 1)
            && call.returnedClientEnvironmentEpoch === index + 1),
        drawUnmounts,
        drawUnmountExactlyOnce: primaryDrawUnmounts.length === 1 && primaryDrawUnmounts[0].handled === true,
      };
    })()`);
    const rootReleased = await run(`!document.querySelector('.mod-ui-above-prompt')`);
    const unmountReleasedRuntimeAndDrawIdentity = rootReleased
      && runtimeLifecycleSnapshot.missingExpectedMounts.length === 0
      && runtimeLifecycleSnapshot.unexpectedMountIds.length === 0
      && runtimeLifecycleSnapshot.missingOrRepeatedUnmounts.length === 0
      && runtimeLifecycleSnapshot.unmountsWithoutMount.length === 0
      && runtimeLifecycleSnapshot.drawUnmountExactlyOnce;

    await run(`window.__modUiClientFixture.startLateRender()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'control' && call.sessionId === 'mod-ui-client-late-render-session' && call.request.subtype === 'ui_render')`);
    await run(`window.__modUiClientFixture.requestLateRerender()`);
    await wait(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.sessionId === 'mod-ui-client-late-render-session' && call.request.subtype === 'ui_render').length === 2`);
    await run(`window.__modUiClientFixture.releaseLateRenderAt(1)`);
    await wait(`document.querySelector('.mod-ui-parent-text')?.textContent.includes('Late revision 101')`);
    await run(`window.__modUiClientFixture.releaseLateRenderAt(0)`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.sessionId === 'mod-ui-client-late-render-session' && call.request.type === 'draw_unmount' && call.request.render_revision === 100)`);
    const staleRenderCleanupPreservedNewerFrame = await run(`document.querySelector('.mod-ui-parent-text')?.textContent.includes('Late revision 101')`);
    await run(`window.__modUiClientFixture.disableLateRender()`);
    await run(`window.__modUiClientFixture.startLateRender()`);
    await wait(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.sessionId === 'mod-ui-client-late-render-session' && call.request.subtype === 'ui_render').length === 3`);
    await run(`window.__modUiClientFixture.disableLateRender()`);
    await delay(60);
    await run(`window.__modUiClientFixture.releaseLateRenderAt(0)`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.sessionId === 'mod-ui-client-late-render-session' && call.request.type === 'draw_unmount' && call.request.render_revision === 102)`);
    await wait(`(() => {
      const lateRenders = window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.sessionId === 'mod-ui-client-late-render-session' && call.request.subtype === 'ui_render');
      const lateUnmounts = new Set(window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation' && call.sessionId === 'mod-ui-client-late-render-session' && call.request.type === 'draw_unmount').map((call) => call.request.render_revision));
      return lateRenders.length > 0 && lateRenders.every((call) => call.returnedRevision !== undefined && lateUnmounts.has(call.returnedRevision));
    })()`);
    const lateInitialRenderCleanedUp = await run(`(() => {
      const lateRenders = window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.sessionId === 'mod-ui-client-late-render-session' && call.request.subtype === 'ui_render');
      const lateUnmounts = new Set(window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation' && call.sessionId === 'mod-ui-client-late-render-session' && call.request.type === 'draw_unmount').map((call) => call.request.render_revision));
      return lateRenders.length > 0 && lateRenders.every((call) => call.returnedRevision !== undefined && lateUnmounts.has(call.returnedRevision));
    })()`);

    process.stdout.write(`${JSON.stringify({ parentElementsRendered, parentButtonRequest, parentInputChangeRequest, parentInputSubmitRequest, parentSelectRequest, parentMarkdownRequest, parentControlRequestsMatchNativeBodies, clientIdentitySnapshot, clientIdentityPropagationMatchesAttach, buttonPressHandled, buttonUsesCurrentHandle, inputUsesHookReachedValue, selectUsesHookReachedValue, preRegistrationFrameReplayed, staleOperationIgnoredWithoutFault, parentPropsUpdateRendered, staleRpcFrameIgnored, staleFrameIgnored, workerFaultRenderedWithoutDuplicateReport, workerFaultDiagnostics, workerFaultLeavesSiblingVisible, workerFaultNotClearedByParentRevision, workerFaultRecoveredFromEnvironmentRemount, asyncWorkerFaultRenderedWithoutDuplicateReport, bufferedAsyncWorkerFaultIsLeafLocal, statusReadyAfterAsyncClientRemoval, statusReadinessTimeline, adapterFaultReportedAsRun, faultLeavesSiblingVisible, adapterRunFaultSurvivedPassiveFrames, adapterRunFaultSurvivedNewRevisionSameIdentity, adapterRunFaultSurvivedSameIdentityPropsUpdate, adapterRunFaultRecoveredAfterStateTokenChange, adapterRunFaultRecoveredAfterClientRemount, pluginUnloadInvalidationRendered, emptyNoHostIsQuietAndCleansUp, runtimeLifecycleSnapshot, unmountReleasedRuntimeAndDrawIdentity, staleRenderCleanupPreservedNewerFrame, lateInitialRenderCleanedUp })}\n`);
  } finally {
    window.destroy();
    app.quit();
  }
}

main().catch((error) => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
