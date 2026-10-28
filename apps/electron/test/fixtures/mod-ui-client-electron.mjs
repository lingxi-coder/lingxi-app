import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';

const url = process.argv.find((argument) => argument.startsWith('http://') || argument.startsWith('file://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function main() {
  await app.whenReady();
  const milestone = (name) => process.stderr.write(`Mod UI fixture: ${name}\n`);
  const window = new BrowserWindow({ show: false, width: 900, height: 600, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const documentFocusedBeforeActivation = await window.webContents.executeJavaScript('document.hasFocus()');
    window.showInactive();
    window.webContents.focus();
    const documentFocusedAfterActivation = await window.webContents.executeJavaScript('document.hasFocus()');
    const run = (script) => window.webContents.executeJavaScript(script);
    const wait = async (expression) => {
      const deadline = Date.now() + 8_000;
      while (Date.now() < deadline) {
        const result = await run(expression);
        if (result) return result;
        await delay(25);
      }
      const diagnostic = await run(`(() => {
        const button = document.querySelector('.mod-ui-client-button');
        const calls = window.__modUiClientFixture?.calls ?? [];
        return {
          documentHasFocus: document.hasFocus(),
          activeElement: document.activeElement?.outerHTML?.slice(0, 180) ?? null,
          button: button ? { connected: button.isConnected, disabled: button.disabled, label: button.getAttribute('aria-label') } : null,
          clickReachedTarget: window.__modUiClientFixture?.clickReachedTarget ?? false,
          clientPresses: calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_client_press').map((call) => ({ request: call.request, result: call.result })),
          runHeld: calls.filter((call) => call.kind === 'operation' && call.request.type === 'runHeld').map((call) => call.request),
          debug: window.__modUiClientFixture?.debugSnapshot?.() ?? null,
        };
      })()`);
      throw new Error(`Timed out: ${expression}\n${JSON.stringify(diagnostic)}\n${await run('document.body.innerText')}`);
    };
    await wait(`window.__modUiClientFixture?.initialMountsHeld()`);
    const rendersBeforeResize = await run(`window.__modUiClientFixture.calls.filter(call => call.kind === 'control' && call.request.subtype === 'ui_render' && call.request.component === 'AbovePrompt').length`);
    await run(`(() => { const site = document.querySelector('.mod-ui-above-prompt'); site.style.height = (Math.ceil(site.getBoundingClientRect().height) + 120) + 'px'; })()`);
    await wait(`window.__modUiClientFixture.calls.filter(call => call.kind === 'control' && call.request.subtype === 'ui_render' && call.request.component === 'AbovePrompt').length > ${rendersBeforeResize}`);
    await run(`window.__modUiClientFixture.releaseInitialMounts()`);
    await wait(`['runtime-status', 'runtime-sibling-1'].every(runtimeId => window.__modUiClientFixture.calls.some(call => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === runtimeId))`);
    await wait(`Boolean(document.querySelector('.mod-ui-client-button')) && document.querySelector('[data-client="sibling"]')?.textContent.includes('Sibling Alive')`);
    const initialStatusMountSuperseded = await run(`window.__modUiClientFixture.debugSnapshot().currentStatusRuntimeId !== 'runtime-status'`);

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
    // Wait until the status resize is actually held. Releasing before its
    // request arrives leaves the subsequent operation queue blocked forever.
    await wait(`window.__modUiClientFixture.initialStatusResizeHeld()`);
    // A viewport update can supersede an in-flight first mount. Track the
    // actual accepted runtime instead of assuming its first generated id.
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'resize' && call.request.runtimeId === window.__modUiClientFixture.debugSnapshot().currentStatusRuntimeId)`);
    const activeRuntime = async (client, epoch) => {
      const expression = `(() => {
        const calls = window.__modUiClientFixture.calls;
        const mount = [...calls].reverse().find(call => call.kind === 'operation' && call.request.type === 'mount'
          && call.request.client === ${JSON.stringify(client)} && call.returnedClientEnvironmentEpoch === ${epoch});
        return mount?.returnedRuntimeId && !calls.some(call => call.kind === 'operation' && call.request.type === 'unmount'
          && call.request.runtimeId === mount.returnedRuntimeId) ? mount.returnedRuntimeId : null;
      })()`;
      await wait(expression);
      return run(expression);
    };
    const initialStatusRuntime = await activeRuntime('status', 1);

    await run(`(() => {
      const button = document.querySelector('.mod-ui-client-button');
      window.__modUiClientFixture.clickReachedTarget = false;
      button.addEventListener('click', (event) => {
        window.__modUiClientFixture.clickReachedTarget = event.target === button;
      }, { once: true });
      button.click();
    })()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'control' && call.request.subtype === 'ui_client_press' && call.request.event.type === 'press')`);
    // The button's host dispatch resolves asynchronously; let its continuation
    // queue behind the held resize before proving it cannot run prematurely.
    await run(`new Promise(resolve => setTimeout(resolve, 0))`);
    assert.equal(await run(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 17)`), false);
    await run(`window.__modUiClientFixture.releaseHeldFrameOperations()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 17)`);
    const buttonPressHandled = await run(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_client_press' && call.request.event.type === 'press').length === 1`);
    const buttonUsesCurrentHandle = await run(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 17 && !('event' in call.request)).length === 1`);

    await run(`(() => {
      const input = document.querySelector('.mod-ui-client-input');
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'typed');
      input.dispatchEvent(new Event('input', { bubbles: true }));
    })()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 18 && call.request.event?.kind === 'change' && call.request.event?.value === 'normalized-input')`);
    const inputUsesHookReachedValue = await run(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 18 && call.request.event?.kind === 'change' && call.request.event?.value === 'normalized-input').length === 1`);

    await run(`(() => {
      document.querySelector('.mod-ui-client-input').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    })()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 18 && call.request.event?.kind === 'submit' && call.request.event?.value === 'normalized-submitted-input')`);
    const inputSubmitUsesHookReachedValue = await run(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 18 && call.request.event?.kind === 'submit' && call.request.event?.value === 'normalized-submitted-input').length === 1`);
    const inputRunHeldEvents = await run(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 18).map((call) => call.request.event)`);

    await run(`(() => {
      const select = document.querySelector('.mod-ui-client-select');
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(select, 'week');
      select.dispatchEvent(new Event('change', { bubbles: true }));
    })()`);
    await wait(`window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 19 && call.request.event?.value === 'week')`);
    const selectUsesHookReachedValue = await run(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 19 && call.request.event?.value === 'week' && Object.keys(call.request.event).length === 1).length === 1`);
    const selectRunHeldEvents = await run(`window.__modUiClientFixture.calls.filter((call) => call.kind === 'operation' && call.request.type === 'runHeld' && call.request.handle === 19).map((call) => call.request.event)`);

    const markdownExternalLinkUsesNormalNavigation = await run(`(() => {
      const anchor = [...document.querySelectorAll('.mod-ui-client-markdown a')].find((item) => item.getAttribute('href') === 'https://example.test/docs');
      return anchor?.getAttribute('target') === '_blank' && anchor?.getAttribute('rel') === 'noreferrer';
    })()`);
    const contextMentionClickPreventedNavigation = await run(`(() => {
      const href = window.__modUiClientFixture.contextMentionHref();
      const anchor = [...document.querySelectorAll('.mod-ui-client-markdown a')].find((item) => item.getAttribute('href') === href);
      return anchor?.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true })) === false;
    })()`);
    const contextMentionHref = await run(`window.__modUiClientFixture.contextMentionHref()`);
    const contextMentionOpenEvents = await run(`window.__modUiClientFixture.contextMentionEvents()`);
    const ordinaryMentionClickPreventedNavigation = await run(`(() => {
      const anchor = [...document.querySelectorAll('.mod-ui-client-markdown a')].find((item) => item.textContent === 'Ordinary mention');
      return anchor?.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true })) === false;
    })()`);
    const ordinaryMentionOpenEvents = await run(`window.__modUiClientFixture.contextMentionEvents()`);
    const markdownOpenLinkUsesNormalNavigation = await run(`(() => {
      const anchor = [...document.querySelectorAll('.mod-ui-client-markdown a')].find((item) => item.getAttribute('href') === 'https://example.test/open');
      return anchor?.getAttribute('target') === '_blank' && anchor?.getAttribute('rel') === 'noreferrer';
    })()`);
    const markdownLinksDoNotDispatchClientCallbacks = await run(`!window.__modUiClientFixture.calls.some((call) => call.kind === 'control' && call.request.subtype === 'ui_client_press' && (call.request.element === 'docs' || call.request.element === 'open-links'))
      && !window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'runHeld' && (call.request.handle === 20 || call.request.handle === 21))`);

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

    const initialSiblingRuntime = await activeRuntime('sibling', 1);
    const initialSiblingMountSuperseded = initialSiblingRuntime !== 'runtime-sibling-1';
    const siblingMountCountBeforeFault = await run(`window.__modUiClientFixture.calls.filter(call => call.kind === 'operation' && call.request.type === 'mount' && call.request.client === 'sibling').length`);
    await run(`window.__modUiClientFixture.failNextRunInWorker(); document.querySelector('.mod-ui-client-button').click()`);
    await wait(`document.querySelector('.mod-ui-client-fallback') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.workerFault === true && call.request.type === 'runHeld' && call.request.handle === 17 && call.request.runtimeId === ${JSON.stringify(initialStatusRuntime)})`);
    await run(`window.__modUiClientFixture.emitHighSequenceFrameForFailedRuntime(${JSON.stringify(initialStatusRuntime)})`);
    await delay(100);
    const workerFaultDiagnostics = await run(`(() => {
      const calls = window.__modUiClientFixture.calls;
      const faultIndex = calls.findIndex((call) => call.kind === 'operation' && call.workerFault === true);
      const postFaultRuntimeOperations = faultIndex < 0 ? [] : calls.slice(faultIndex + 1).filter((call) => call.kind === 'operation'
        && call.request.runtimeId === ${JSON.stringify(initialStatusRuntime)}
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
      const siblingUnmounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === ${JSON.stringify(initialSiblingRuntime)});
      return Boolean(document.querySelector('[data-client="sibling"]'))
        && document.querySelector('[data-client="sibling"]').textContent.includes('Sibling Alive')
        && siblingMounts.length === ${siblingMountCountBeforeFault}
        && siblingMounts.at(-1)?.returnedRuntimeId === ${JSON.stringify(initialSiblingRuntime)}
        && siblingMounts.at(-1)?.returnedClientEnvironmentEpoch === 1
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
        && call.request.runtimeId === ${JSON.stringify(initialStatusRuntime)}
        && ['render', 'setProps', 'resize', 'pointer', 'key', 'runHeld'].includes(call.request.type));
      const statusMounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'mount' && call.request.client === 'status');
      return document.querySelector('.mod-ui-client-fallback') !== null
        && statusMounts.length === ${statusMountCountBeforeOrdinaryRevision}
        && sameRuntimeCalls.length === 0;
    })()`);

    await run(`window.__modUiClientFixture.reloadStatusPluginEnvironment()`);
    await wait(`document.querySelector('.mod-ui-client-surface')?.textContent.includes('Recovered after environment remount')`);
    const statusEpoch2Runtime = await activeRuntime('status', 2);
    const siblingEpoch2Runtime = await activeRuntime('sibling', 2);
    const siblingMountCountBeforeAsyncFault = await run(`window.__modUiClientFixture.calls.filter(call => call.kind === 'operation' && call.request.type === 'mount' && call.request.client === 'sibling').length`);
    const workerFaultRecoveredFromEnvironmentRemount = await run(`!document.querySelector('.mod-ui-client-fallback') && document.querySelector('.mod-ui-client-surface')?.textContent.includes('Recovered after environment remount') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === ${JSON.stringify(initialStatusRuntime)})`);

    await run(`window.__modUiClientFixture.emitAsyncWorkerFault()`);
    await wait(`document.querySelector('.mod-ui-client-fallback') !== null`);
    await run(`window.__modUiClientFixture.emitHighSequenceFrameForFailedRuntime(${JSON.stringify(statusEpoch2Runtime)})`);
    await delay(80);
    const asyncWorkerFaultRenderedWithoutDuplicateReport = await run(`(() => {
      const calls = window.__modUiClientFixture.calls;
      const siblingMounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'mount' && call.request.client === 'sibling');
      const currentSiblingUnmounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === ${JSON.stringify(siblingEpoch2Runtime)});
      return document.querySelector('.mod-ui-client-fallback') !== null
        && document.querySelector('[data-client="sibling"]')?.textContent.includes('Sibling Alive')
        && siblingMounts.length === ${siblingMountCountBeforeAsyncFault}
        && siblingMounts.at(-1)?.returnedRuntimeId === ${JSON.stringify(siblingEpoch2Runtime)}
        && siblingMounts.at(-1)?.returnedClientEnvironmentEpoch === 2
        && currentSiblingUnmounts.length === 0
        && calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_client_fault').length === 0;
    })()`);

    await run(`window.__modUiClientFixture.reloadStatusPluginEnvironment()`);
    await wait(`document.querySelector('.mod-ui-client-surface')?.textContent.includes('Recovered after environment remount')`);
    const statusEpoch3Runtime = await activeRuntime('status', 3);
    const siblingEpoch3Runtime = await activeRuntime('sibling', 3);

    await run(`window.__modUiClientFixture.enableAsyncFaultClient()`);
    await wait(`document.querySelector('[data-client="async-fault"] .mod-ui-client-fallback') !== null`);
    const asyncFaultRuntime = await activeRuntime('async-fault', 3);
    const bufferedAsyncWorkerFaultIsLeafLocal = await run(`document.querySelector('[data-client="async-fault"] .mod-ui-client-fallback') !== null
      && document.querySelector('[data-client="status"] .mod-ui-client-fallback') === null
      && document.querySelector('[data-client="sibling"]')?.textContent.includes('Sibling Alive')
      && !document.querySelector('[data-client="async-fault"] .mod-ui-client-surface')?.textContent.includes('Buffered pre-fault frame')
      && window.__modUiClientFixture.calls.filter((call) => call.kind === 'control' && call.request.subtype === 'ui_client_fault').length === 0`);
    await run(`window.__modUiClientFixture.disableAsyncFaultClient()`);
    await wait(`!document.querySelector('[data-client="async-fault"]') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === ${JSON.stringify(asyncFaultRuntime)})`);
    const statusRuntimeAfterAsyncRemoval = await activeRuntime('status', 3);
    const statusReadyAfterAsyncClientRemovalExpression = `(() => {
      const calls = window.__modUiClientFixture.calls;
      const latestRender = [...calls].reverse().find((call) => call.kind === 'control'
        && call.sessionId === 'mod-ui-client-fixture-session'
        && call.request.subtype === 'ui_render'
        && call.request.component === 'AbovePrompt'
        && call.returnedRevision !== undefined);
      return latestRender !== undefined && calls.some((call) => call.kind === 'operation'
        && call.request.type === 'render'
        && call.request.runtimeId === ${JSON.stringify(statusRuntimeAfterAsyncRemoval)}
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
          && (call.request.type === 'draw_commit' || (call.request.type === 'render' && call.request.runtimeId === ${JSON.stringify(statusRuntimeAfterAsyncRemoval)}))
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
    const statusReadyAfterAsyncClientRemoval = statusRuntimeAfterAsyncRemoval === statusEpoch3Runtime && await run(statusReadyAfterAsyncClientRemovalExpression);
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

    const statusRuntimeBeforeUnload = await activeRuntime('status', 3);
    await run(`window.__modUiClientFixture.unloadStatusPlugin()`);
    await wait(`!document.querySelector('.mod-ui-client-instance[data-client="status"]') && document.querySelector('[data-client="sibling"]')?.textContent.includes('Sibling Alive') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === ${JSON.stringify(statusRuntimeBeforeUnload)})`);
    await run(`window.__modUiClientFixture.loadStatusPlugin()`);
    await wait(`document.querySelector('[data-client="status"] .mod-ui-client-surface')?.textContent.includes('Recovered after environment remount')`);
    const reloadedStatusRuntime = await activeRuntime('status', 3);
    const adapterRunFaultRecoveredAfterClientRemount = statusRuntimeBeforeUnload === statusEpoch3Runtime && reloadedStatusRuntime !== statusRuntimeBeforeUnload && await run(`document.querySelector('[data-client="status"] .mod-ui-client-fallback') === null && document.querySelector('[data-client="status"] .mod-ui-client-surface')?.textContent.includes('Recovered after environment remount')`);

    await run(`window.__modUiClientFixture.unloadStatusPlugin()`);
    await wait(`!document.querySelector('.mod-ui-client-instance[data-client="status"]') && document.querySelector('[data-client="sibling"]')?.textContent.includes('Sibling Alive') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === ${JSON.stringify(reloadedStatusRuntime)})`);
    const pluginUnloadInvalidationRendered = await run(`!document.querySelector('.mod-ui-client-instance[data-client="status"]') && Boolean(document.querySelector('[data-client="sibling"]')) && document.querySelector('[data-client="sibling"]').textContent.includes('Sibling Alive') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'unmount' && call.request.runtimeId === ${JSON.stringify(reloadedStatusRuntime)})`);

    await run(`window.__modUiClientFixture.showNoMods()`);
    await wait(`!document.querySelector('.mod-ui-client-instance') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'draw_unmount')`);
    const emptyNoHostIsQuietAndCleansUp = await run(`!document.querySelector('.mod-ui-client-fallback') && !document.querySelector('.mod-ui-client-instance') && window.__modUiClientFixture.calls.some((call) => call.kind === 'operation' && call.request.type === 'draw_unmount')`);

    await run(`window.__modUiClientFixture.disable()`);
    await wait(`!document.querySelector('.mod-ui-above-prompt')`);
    await delay(80);
    const expectedMounts = [
      { reason: 'status superseded by the initial viewport update', client: 'status', epoch: 1, runtimeId: 'runtime-status' },
      { reason: 'status after the initial viewport update', client: 'status', epoch: 1, runtimeId: initialStatusRuntime },
      { reason: 'sibling superseded by the initial viewport update', client: 'sibling', epoch: 1, runtimeId: 'runtime-sibling-1' },
      { reason: 'sibling after the initial viewport update', client: 'sibling', epoch: 1, runtimeId: initialSiblingRuntime },
      { reason: 'status after first environment reload', client: 'status', epoch: 2, runtimeId: statusEpoch2Runtime },
      { reason: 'sibling after first environment reload', client: 'sibling', epoch: 2, runtimeId: siblingEpoch2Runtime },
      { reason: 'status after second environment reload', client: 'status', epoch: 3, runtimeId: statusEpoch3Runtime },
      { reason: 'sibling after second environment reload', client: 'sibling', epoch: 3, runtimeId: siblingEpoch3Runtime },
      { reason: 'explicit async fault client', client: 'async-fault', epoch: 3, runtimeId: asyncFaultRuntime },
      { reason: 'status after plugin unload and reload', client: 'status', epoch: 3, runtimeId: reloadedStatusRuntime },
    ];
    const runtimeLifecycleSnapshot = await run(`(() => {
      const calls = window.__modUiClientFixture.calls.filter((call) => call.sessionId === 'mod-ui-client-fixture-session');
      const mountCalls = calls.filter((call) => call.kind === 'operation' && call.request.type === 'mount');
      const mounts = mountCalls.map((call) => call.returnedRuntimeId);
      const unmounts = calls.filter((call) => call.kind === 'operation' && call.request.type === 'unmount').map((call) => call.request.runtimeId);
      const expected = ${JSON.stringify(expectedMounts)};
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
        expectedMounts: expected,
        mountedRuntimeIds: mounts,
        unmountedRuntimeIds: unmounts,
        mountCounts,
        unmountCounts,
        duplicateExpectedRuntimeIds: expected.filter((item, index) => !item.runtimeId
          || expected.findIndex((other) => other.runtimeId === item.runtimeId) !== index).map((item) => item.runtimeId),
        missingExpectedMounts: expected.filter((item) => mountCounts[item.runtimeId] !== 1
          || !mountCalls.some((call) => call.returnedRuntimeId === item.runtimeId
            && call.request.client === item.client && call.request.plugin === 'fixture-plugin'
            && call.returnedClientEnvironmentEpoch === item.epoch)),
        unexpectedMountIds: mountCalls.filter((call) => !expected.some((item) => call.returnedRuntimeId === item.runtimeId
          && call.request.client === item.client && call.request.plugin === 'fixture-plugin'
          && call.returnedClientEnvironmentEpoch === item.epoch)).map((call) => call.returnedRuntimeId),
        missingOrRepeatedUnmounts: expected.filter((item) => unmountCounts[item.runtimeId] !== 1).map((item) => item.runtimeId),
        unmountsWithoutMount: unmounts.filter((runtimeId) => mountCounts[runtimeId] !== 1),
        siblingMountsByPluginEpoch: siblingMounts.map((call) => ({ plugin: call.request.plugin, runtimeId: call.returnedRuntimeId, epoch: call.returnedClientEnvironmentEpoch })),
        pluginEnvironmentRemountsAllClients: siblingMounts.length === 4
          && siblingMounts.every((call, index) => call.request.plugin === 'fixture-plugin'
            && call.returnedRuntimeId === ${JSON.stringify(['runtime-sibling-1', initialSiblingRuntime, siblingEpoch2Runtime, siblingEpoch3Runtime])}[index]
            && call.returnedClientEnvironmentEpoch === [1, 1, 2, 3][index]),
        drawUnmounts,
        drawUnmountExactlyOnce: primaryDrawUnmounts.length === 1 && primaryDrawUnmounts[0].handled === true,
      };
    })()`);
    const rootReleased = await run(`!document.querySelector('.mod-ui-above-prompt')`);
    const unmountReleasedRuntimeAndDrawIdentity = rootReleased
      && runtimeLifecycleSnapshot.duplicateExpectedRuntimeIds.length === 0
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

    milestone('initial surfaces complete; starting rapid input');
    await run(`window.__modUiClientFixture.holdRapidPostAt(1); window.__modUiClientFixture.holdRapidPostAt(2); window.__modUiClientFixture.startRapidInputScenario('identity')`);
    await wait(`Boolean(document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')) && Boolean(document.querySelector('.beta-rich-prompt'))`);
    await run(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input').focus()`);
    const sendText = async (text, expectInput = true) => {
      const deliveredValues = [];
      for (const character of text) {
        // sendInputEvent queues native delivery. Do not queue the next key
        // until this key has produced its trusted renderer event: otherwise a
        // busy host can coalesce the fixture's synthetic burst before React
        // observes each value, invalidating the per-prefix assertions below.
        await run(`(() => {
          window.__rapidCharacterDelivered = null;
          const input = document.querySelector('[data-client="rapid-input"] .mod-ui-client-input');
          input.addEventListener(${JSON.stringify(expectInput ? 'input' : 'keydown')}, event => {
            if (event.isTrusted && (${expectInput} || event.key === ${JSON.stringify(character)})) {
              window.__rapidCharacterDelivered = { value: input.value };
            }
          }, { once: true });
        })()`);
        const physicalKey = character === '-' ? '-' : character.toUpperCase();
        window.webContents.sendInputEvent({ type: 'keyDown', keyCode: physicalKey });
        window.webContents.sendInputEvent({ type: 'char', keyCode: character });
        window.webContents.sendInputEvent({ type: 'keyUp', keyCode: physicalKey });
        const delivered = await wait(`window.__rapidCharacterDelivered`);
        if (expectInput) deliveredValues.push(delivered.value);
      }
      return deliveredValues;
    };
    const sendEnter = () => {
      window.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'ENTER' });
      window.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'ENTER' });
    };
    const clearRapidDraft = async () => {
      const initial = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
      if (initial.value === '') return;
      const before = initial.rapidOnInputCallbacks.length;
      await run(`(() => { const input = document.querySelector('[data-client="rapid-input"] .mod-ui-client-input');
        input.focus(); input.select(); window.__clearRapidTrusted = false;
        input.addEventListener('input', event => { window.__clearRapidTrusted = event.isTrusted; }, { once: true }); })()`);
      window.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'BACKSPACE' });
      window.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'BACKSPACE' });
      await wait(`window.__clearRapidTrusted`);
      await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot();
        return s.value === '' && s.rapidPropValue === '' && s.rapidOnInputCallbacks.length === ${before + 1}; })()`);
    };
    const assertUnconfirmedEnterBlocked = async () => {
      const before = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
      await run(`(() => {
        window.__unconfirmedEnterDelivered = false;
        document.querySelector('[data-client="rapid-input"] .mod-ui-client-input').addEventListener('keydown', event => {
          window.__unconfirmedEnterDelivered = event.isTrusted && event.key === 'Enter';
        }, { once: true });
      })()`);
      sendEnter();
      await wait(`window.__unconfirmedEnterDelivered`);
      await delay(80);
      const after = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
      assert.equal(after.controlRequests.length, before.controlRequests.length,
        'Enter must not dispatch an unconfirmed change or submit');
      assert.deepEqual(after.rapidOnSubmitCallbacks, before.rapidOnSubmitCallbacks,
        'Enter must not run the submit callback for an unconfirmed action');
      assert.match(after.unconfirmedNotice, /not confirmed/i);
    };
    const retryUnconfirmed = async (label) => {
      assert.equal((await run(`window.__modUiClientFixture.rapidInputSnapshot()`)).retryLabel, label);
      await run(`(() => {
        window.__retryClickTrusted = false;
        const button = document.querySelector('[data-client="rapid-input"] .mod-ui-client-input-retry');
        button.addEventListener('click', event => { window.__retryClickTrusted = event.isTrusted; }, { once: true });
        button.focus();
      })()`);
      window.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'ENTER' });
      window.webContents.sendInputEvent({ type: 'char', keyCode: '\r' });
      window.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'ENTER' });
      await wait(`window.__retryClickTrusted`);
    };
    const rapidTarget = 'packaged-host-cua-valuexy';
    // A resize has already replaced the worker's held handles but its frame
    // response is queued ahead of this input. The event must resolve its
    // handle only when the resize response has committed to the queue.
    await run(`(() => { window.__modUiClientFixture.holdNextRapidResize();
      const surface = document.querySelector('[data-client="rapid-input"]');
      surface.style.width = Math.max(100, Math.floor(surface.getBoundingClientRect().width) - 120) + 'px'; })()`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidResizeHeld`);
    const trustedRapidValues = await sendText(rapidTarget[0]);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().controlRequests.length === 1`);
    await run(`new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
    assert.equal((await run(`window.__modUiClientFixture.rapidInputSnapshot()`)).runHeld.length, 0,
      'the held callback must wait behind the in-flight resize');
    await run(`window.__modUiClientFixture.releaseRapidResize()`);
    await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot();
      return s.rapidPostOrdinal === 1 || s.rapidRejectedHandles > 0; })()`);
    const firstQueuedInput = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(firstQueuedInput.rapidRejectedHandles, 0, 'an older held handle must not be sent after the newer resize frame');
    assert.deepEqual(firstQueuedInput.rapidInputCallbacks, [{ kind: 'change', value: rapidTarget[0] }]);
    trustedRapidValues.push(...await sendText(rapidTarget.slice(1)));
    assert.deepEqual(trustedRapidValues, [...rapidTarget].map((_, index) => rapidTarget.slice(0, index + 1)),
      'each trusted input event must reach React with its own prefix');
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === '${rapidTarget}' && window.__modUiClientFixture.rapidInputSnapshot().rapidPostOrdinal === 1`, 'rapid local edits while first Host post is delayed');
    await run(`(() => {
      window.__rapidEnterDelivered = false;
      document.querySelector('[data-client="rapid-input"] .mod-ui-client-input').addEventListener('keydown', event => {
        window.__rapidEnterDelivered = event.isTrusted && event.key === 'Enter';
      }, { once: true });
    })()`);
    sendEnter();
    // Native input delivery is asynchronous. Observe Enter reaching the input
    // before injecting a DOM click, preserving the intended user action order
    // while the first Host post remains held.
    await wait(`window.__rapidEnterDelivered === true`);
    await run(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-button').click()`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().controlRequests.length === 1 && window.__modUiClientFixture.rapidInputSnapshot().runHeld.length === 1`, 'queued events wait for the first settled post');
    const rapidBeforeRelease = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(rapidBeforeRelease.value, rapidTarget);
    assert.equal(rapidBeforeRelease.clientFocused, true);
    assert.equal(rapidBeforeRelease.promptText, '');
    assert.equal(rapidBeforeRelease.mainPromptSubmitCount, 0);
    assert.equal(rapidBeforeRelease.controlRequests.length, 1);
    assert.equal(rapidBeforeRelease.runHeld.length, 1);

    await run(`window.__modUiClientFixture.releaseRapidPostAt(1)`);
    const rapidAfterFirstAck = await wait(`(() => {
      const state = window.__modUiClientFixture.rapidInputSnapshot();
      return state.rapidPostOrdinal === 2 && state.rapidInputCallbacks.length === 2 ? state : null;
    })()`);
    assert.equal(rapidAfterFirstAck.rapidInputValue, rapidTarget.slice(0, 2), JSON.stringify(rapidAfterFirstAck));
    assert.equal(rapidAfterFirstAck.rapidPropValue, rapidTarget[0]);
    assert.deepEqual(rapidAfterFirstAck.rapidBackgroundPending, [{ ordinal: 2, value: rapidTarget.slice(0, 2) }]);
    assert.equal(rapidAfterFirstAck.value, rapidTarget, 'the first value echo must not replace the newer local draft');
    assert.equal(rapidAfterFirstAck.clientFocused, true);
    assert.equal(rapidAfterFirstAck.mainPromptSubmitCount, 0);

    const beforeSecondRelease = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(beforeSecondRelease.rapidPostOrdinal, 2, 'no later callback may run while the second post is held');
    assert.equal(beforeSecondRelease.rapidInputCallbacks.length, 2);
    await run(`window.__modUiClientFixture.releaseRapidPostAt(2)`);
    await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot(); return s.rapidControlOrdinal === ${rapidTarget.length + 2} && s.rapidInputCallbacks.length === ${rapidTarget.length + 1} && s.rapidButtonSubmitCount === 1 && s.rapidInputResult.includes('Button accepted: ${rapidTarget}'); })()`, 'all queued input, submit, and Button callbacks complete');
    const rapidInputRegression = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    const rapidPrefixes = [...rapidTarget].map((_, index) => rapidTarget.slice(0, index + 1));
    assert.deepEqual(rapidInputRegression.rapidInputCallbacks, [
      ...rapidPrefixes.map((value) => ({ kind: 'change', value })),
      { kind: 'submit', value: rapidTarget },
    ]);
    assert.deepEqual(rapidInputRegression.rapidOnInputCallbacks, rapidPrefixes);
    assert.deepEqual(rapidInputRegression.rapidOnSubmitCallbacks, [rapidTarget]);
    assert.deepEqual(rapidInputRegression.rapidButtonCallbacks, [rapidTarget]);
    assert.deepEqual(rapidInputRegression.controlRequests.slice(0, rapidTarget.length).map((event) => event.value), rapidPrefixes);
    assert.equal(rapidInputRegression.controlRequests.at(-2).kind, 'submit', JSON.stringify(rapidInputRegression.controlRequests.slice(-3)));
    assert.equal(rapidInputRegression.controlRequests.at(-1).type, 'press');
    const rapidHeldHandles = rapidInputRegression.runHeld.map(({ handle }) => handle);
    assert.equal(new Set(rapidHeldHandles).size, rapidHeldHandles.length, 'each queued event must use the latest frame handle');
    assert.equal(rapidInputRegression.value, rapidTarget);
    assert.equal(rapidInputRegression.clientFocused, true);
    assert.equal(rapidInputRegression.promptText, '');
    assert.equal(rapidInputRegression.mainPromptSubmitCount, 0);
    milestone('rapid input and queued-handle race complete');

    // If the reached element disappears while an earlier resize occupies the
    // operation queue, the retired handle must never target another node.
    await run(`window.__modUiClientFixture.emitRapidExternalValue('')`);
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === ''`);
    const beforeRemovedTarget = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    await run(`(() => { window.__modUiClientFixture.holdNextRapidResize();
      const surface = document.querySelector('[data-client="rapid-input"]');
      surface.style.width = Math.max(100, Math.floor(surface.getBoundingClientRect().width) - 80) + 'px'; })()`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidResizeHeld`);
    await sendText('r');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().controlRequests.length === ${beforeRemovedTarget.controlRequests.length + 1}`);
    await run(`new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
    await run(`window.__modUiClientFixture.removeRapidInput()`);
    await wait(`!document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')`);
    await run(`window.__modUiClientFixture.releaseRapidResize(); new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
    const afterRemovedTarget = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(afterRemovedTarget.runHeld.length, beforeRemovedTarget.runHeld.length,
      'a removed target must not dispatch a retired or unrelated handle');
    assert.deepEqual(afterRemovedTarget.rapidInputCallbacks, beforeRemovedTarget.rapidInputCallbacks);
    await run(`window.__modUiClientFixture.restoreRapidInput('')`);
    await wait(`Boolean(document.querySelector('[data-client="rapid-input"] .mod-ui-client-input'))`);
    await run(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input').focus()`);
    milestone('removed target race complete');

    let callbackCount = rapidInputRegression.rapidInputCallbacks.length;
    await run(`window.__modUiClientFixture.setRapidInputMode('uppercase'); window.__modUiClientFixture.emitRapidExternalValue('')`);
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === ''`);
    await sendText('a');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidInputCallbacks.length === ${callbackCount + 1} && document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === 'A'`);
    await sendText('b');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidInputCallbacks.length === ${callbackCount + 2} && document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === 'AB'`, 'author-transformed input is acknowledged from the settled frame');
    const transformedValue = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.deepEqual(transformedValue.rapidInputCallbacks.slice(callbackCount), [
      { kind: 'change', value: 'a' }, { kind: 'change', value: 'Ab' },
    ]);
    assert.equal(transformedValue.rapidPropValue, 'AB');
    callbackCount += 2;

    await run(`window.__modUiClientFixture.emitRapidExternalValue('external-after-transform')`);
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === 'external-after-transform'`, 'unlocked input accepts a later external value');
    await run(`window.__modUiClientFixture.setRapidInputMode('clear-on-submit'); window.__modUiClientFixture.emitRapidExternalValue('')`);
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === ''`);
    await sendText('x');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidInputCallbacks.length === ${callbackCount + 1} && document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === 'x'`);
    sendEnter();
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidInputCallbacks.length === ${callbackCount + 2} && document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === ''`, 'an authoritative empty string clears the draft');
    const clearedValue = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.deepEqual(clearedValue.rapidInputCallbacks.slice(callbackCount), [
      { kind: 'change', value: 'x' }, { kind: 'submit', value: 'x' },
    ]);
    callbackCount += 2;

    await run(`window.__modUiClientFixture.setRapidInputMode('uncontrolled'); window.__modUiClientFixture.emitRapidExternalValue(undefined)`);
    await sendText('draft');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidInputCallbacks.length === ${callbackCount + 5} && document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === 'draft'`, 'uncontrolled acknowledgement preserves the local draft');
    const uncontrolledValue = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(uncontrolledValue.rapidPropValue, undefined);
    assert.deepEqual(uncontrolledValue.rapidInputCallbacks.slice(callbackCount), [
      { kind: 'change', value: 'd' }, { kind: 'change', value: 'dr' }, { kind: 'change', value: 'dra' },
      { kind: 'change', value: 'draf' }, { kind: 'change', value: 'draft' },
    ]);
    callbackCount += 5;
    await run(`window.__modUiClientFixture.emitRapidExternalValue('external-after-uncontrolled')`);
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === 'external-after-uncontrolled'`, 'uncontrolled local edit does not retain a stale lock');

    await run(`window.__modUiClientFixture.setRapidInputMode('identity'); window.__modUiClientFixture.emitRapidExternalValue(''); window.__modUiClientFixture.ignoreNextRapidRunAsStale()`);
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === ''`);
    await sendText('z');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().retryLabel === 'Retry change'`);
    const staleChange = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(staleChange.value, 'z', 'a noop must preserve the draft for explicit retry');
    assert.equal(staleChange.rapidInputCallbacks.length, callbackCount, 'a noop must not be retried automatically');
    for (let attempt = 0; attempt < 2; attempt += 1) {
      await run(`window.__modUiClientFixture.ignoreNextRapidRunAsStale()`);
      const beforeAttempt = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
      await retryUnconfirmed('Retry change');
      await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot();
        return s.runHeld.length === ${beforeAttempt.runHeld.length + 1}
          && document.querySelector('[data-client="rapid-input"] .mod-ui-client-input-retry')?.getAttribute('aria-busy') !== 'true'; })()`);
      const afterAttempt = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
      assert.equal(afterAttempt.retryLabel, 'Retry change', 'each noop must stay unconfirmed');
      assert.equal(afterAttempt.value, 'z');
      assert.equal(afterAttempt.rapidInputCallbacks.length, callbackCount);
    }
    await retryUnconfirmed('Retry change');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().unconfirmedNotice === null`);
    assert.deepEqual((await run(`window.__modUiClientFixture.rapidInputSnapshot()`)).rapidInputCallbacks.at(-1),
      { kind: 'change', value: 'z' });
    callbackCount += 1;
    await run(`window.__modUiClientFixture.emitRapidExternalValue('external-after-stale')`);
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === 'external-after-stale'`, 'external props apply after a stale dispatch');

    // Older Hosts may return a noop after accepting a callback. A timeout can
    // do the same. Neither outcome permits an automatic second invocation.
    for (const outcome of ['old-noop', 'timeout-after-acceptance']) {
      await run(`window.__modUiClientFixture.emitRapidExternalValue('')`);
      await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === ''`);
      const beforeUnknown = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
      await sendText('o');
      await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidOnInputCallbacks.length === ${beforeUnknown.rapidOnInputCallbacks.length + 1}`);
      await run(outcome === 'old-noop' ? `window.__modUiClientFixture.oldHostNoopAfterNextSubmit()`
        : `window.__modUiClientFixture.failAfterNextSubmit()`);
      sendEnter();
      await wait(`window.__modUiClientFixture.rapidInputSnapshot().retryLabel === 'Retry submit'`);
      const unknown = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
      assert.equal(unknown.value, 'o');
      assert.equal(unknown.originalSubmitValue, '"o"');
      assert.match(unknown.unconfirmedNotice, /may already have run/i);
      assert.equal(unknown.rapidOnSubmitCallbacks.length, beforeUnknown.rapidOnSubmitCallbacks.length + 1);
      await delay(100);
      assert.equal((await run(`window.__modUiClientFixture.rapidInputSnapshot()`)).runHeld.length, unknown.runHeld.length,
        `${outcome} must not automatically replay an accepted submit`);
      await assertUnconfirmedEnterBlocked();
      await retryUnconfirmed('Retry submit');
      await wait(`window.__modUiClientFixture.rapidInputSnapshot().unconfirmedNotice === null`);
      const explicitRetry = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
      assert.equal(explicitRetry.rapidOnSubmitCallbacks.length, beforeUnknown.rapidOnSubmitCallbacks.length + 2,
        'a retry occurs only after the user explicitly accepts the warning');
      assert.deepEqual(explicitRetry.rapidInputCallbacks.at(-1), { kind: 'submit', value: 'o' });
      callbackCount += 3;
    }
    milestone('noop, explicit retries, and ambiguous accepted results complete');

    // Inject a newer host frame after the old runHeld frame is accepted but
    // before its acknowledgement is consumed by the Input component.
    await run(`window.__modUiClientFixture.emitRapidExternalValue('')`);
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === ''`);
    const beforeFrameRace = await run(`window.__modUiClientFixture.rapidInputSnapshot().rapidInputCallbacks.length`);
    await run(`window.__modUiClientFixture.raceNextRapidAck('newer-host-frame')`);
    await sendText('q');
    await wait(`(() => {
      const state = window.__modUiClientFixture.rapidInputSnapshot();
      return state.rapidAckRaceFired && state.rapidInputCallbacks.length === ${beforeFrameRace + 1}
        && state.rapidPropValue === 'newer-host-frame' && state.value === 'newer-host-frame';
    })()`);

    // Keep one callback in flight and another queued while the parent moves
    // to N+1. The old tree remains visible until the N+1 frame is ready.
    await run(`window.__modUiClientFixture.emitRapidExternalValue('')`);
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === ''`);
    const beforeRevision = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    const heldRevisionPost = beforeRevision.rapidPostOrdinal + 1;
    await run(`window.__modUiClientFixture.captureRapidInput(); window.__modUiClientFixture.holdRapidPostAt(${heldRevisionPost})`);
    await sendText('qr');
    await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot();
      return s.value === 'qr' && s.rapidBackgroundPending.some(p => p.ordinal === ${heldRevisionPost}); })()`);
    await run(`window.__modUiClientFixture.invalidateRapidSite('new-revision-value')`);
    await wait(`window.__modUiClientFixture.calls.some(call => call.kind === 'operation'
      && call.sessionId === 'mod-ui-client-rapid-input-session' && call.request.type === 'draw_commit'
      && call.request.render_revision === 201 && call.returnedRevision === 201)`);
    const revisionPending = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(revisionPending.sameInputNode, true, 'parent revision must keep the same Input DOM node');
    assert.equal(revisionPending.clientFocused, true, 'pending revision must retain Input focus');
    assert.equal(revisionPending.value, 'qr', 'pending revision must retain the local draft');
    assert.equal(revisionPending.inputReadOnly, true, 'an old frame must not accept new input');
    assert.match(revisionPending.unconfirmedNotice, /not confirmed/i);
    await run(`(() => { window.__revisionKeyDelivered = false;
      document.querySelector('[data-client="rapid-input"] .mod-ui-client-input').addEventListener('keydown',
        event => { window.__revisionKeyDelivered = event.isTrusted && event.key === 't'; }, { once: true }); })()`);
    await sendText('t', false);
    await wait(`window.__revisionKeyDelivered === true`);
    const whileReadOnly = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(whileReadOnly.value, 'qr');
    assert.equal(whileReadOnly.controlRequests.length, beforeRevision.controlRequests.length + 1);
    await run(`window.__modUiClientFixture.releaseRapidPostAt(${heldRevisionPost})`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().revisionFrameHeld`);
    const afterOldAck = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(afterOldAck.sameInputNode, true);
    assert.equal(afterOldAck.clientFocused, true);
    assert.equal(afterOldAck.value, 'qr', 'the old acknowledgement cannot overwrite the draft');
    assert.equal(afterOldAck.controlRequests.length, beforeRevision.controlRequests.length + 1,
      'a queued old-revision event must not be sent while the new frame is pending');
    await run(`window.__modUiClientFixture.releaseRapidRevisionFrame()`);
    await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot();
      return s.inputReadOnly === false && s.rapidPropValue === 'new-revision-value'; })()`);
    const revisionReady = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(revisionReady.sameInputNode, true);
    assert.equal(revisionReady.clientFocused, true);
    assert.equal(revisionReady.value, 'qr', 'an unconfirmed draft must remain visible after the new frame');
    assert.match(revisionReady.unconfirmedNotice, /not confirmed/i);
    assert.equal(revisionReady.controlRequests.length, beforeRevision.controlRequests.length + 1,
      'old queued input must not be replayed implicitly');
    await assertUnconfirmedEnterBlocked();
    await retryUnconfirmed('Retry change');
    await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot();
      return s.rapidInputCallbacks.length === ${beforeRevision.rapidInputCallbacks.length + 2}
        && s.unconfirmedNotice === null; })()`);
    const revisionRetry = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.deepEqual(revisionRetry.rapidInputCallbacks.slice(beforeRevision.rapidInputCallbacks.length), [
      { kind: 'change', value: 'q' }, { kind: 'change', value: 'qr' },
    ]);
    assert.deepEqual(revisionRetry.rapidOnSubmitCallbacks, beforeRevision.rapidOnSubmitCallbacks);
    assert.equal(revisionRetry.runHeld.at(-1).revision, 201, 'an explicit retry must reach the new revision');
    assert.equal(revisionRetry.value, 'qr');
    assert.equal(revisionRetry.clientFocused, true, 'a completed retry must return focus to the input');
    milestone('revision change and retry complete');

    // Advance the Host revision while deliberately holding React's parent
    // commit. A late control result in this window must not roll back the
    // draft before the component can observe the new revision in its props.
    await run(`window.__modUiClientFixture.emitRapidExternalValue('')`);
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === ''`);
    await run(`window.__modUiClientFixture.captureRapidInput(); window.__modUiClientFixture.holdNextRapidControl()`);
    const beforeParentCommit = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    await sendText('p');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidControlHeld`);
    await run(`window.__holdRapidParentCommit = true; window.__modUiClientFixture.invalidateRapidSite('precommit-host-value')`);
    await wait(`typeof window.__heldRapidParentCommit === 'function'`);
    await run(`window.__modUiClientFixture.releaseRapidControl()`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().unconfirmedNotice !== null`);
    const beforeReactCommit = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(beforeReactCommit.sameInputNode, true);
    assert.equal(beforeReactCommit.clientFocused, true);
    assert.equal(beforeReactCommit.value, 'p', 'a superseded control result cannot reset an uncommitted draft');
    assert.equal(beforeReactCommit.runHeld.length, beforeParentCommit.runHeld.length,
      'a result from the old Host revision must not start runHeld');
    await run(`window.__holdRapidParentCommit = false; const commit = window.__heldRapidParentCommit;
      window.__heldRapidParentCommit = null; commit()`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().revisionFrameHeld`);
    await run(`window.__modUiClientFixture.releaseRapidRevisionFrame()`);
    await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot();
      return s.inputReadOnly === false && s.rapidPropValue === 'precommit-host-value'; })()`);
    const afterParentCommit = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(afterParentCommit.value, 'p');
    assert.match(afterParentCommit.unconfirmedNotice, /not confirmed/i);
    await retryUnconfirmed('Retry change');
    await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot();
      return s.rapidInputCallbacks.length === ${beforeParentCommit.rapidInputCallbacks.length + 1}
        && s.unconfirmedNotice === null; })()`);
    const precommitRetry = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.deepEqual(precommitRetry.rapidInputCallbacks.at(-1), { kind: 'change', value: 'p' });
    assert.deepEqual(precommitRetry.rapidOnSubmitCallbacks, beforeParentCommit.rapidOnSubmitCallbacks);
    assert.equal(precommitRetry.runHeld.at(-1).revision, beforeParentCommit.rapidRevision + 1);

    // A rejected runHeld promise belongs to the revision that started it.
    // Exercise both the Host-advanced/React-uncommitted window and the window
    // after React has committed the replacement parent revision.
    const staleRunWindows = process.env.LINGXI_TEST_STALE_RUN_WINDOW === 'committed' ? [false] : [true, false];
    for (const holdParentCommit of staleRunWindows) {
      await run(`window.__modUiClientFixture.emitRapidExternalValue('')`);
      await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === ''`);
      const beforeRejection = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
      const faultsBefore = await run(`window.__modUiClientFixture.calls.filter(call => call.kind === 'control'
        && call.request.subtype === 'ui_client_fault' && call.sessionId === 'mod-ui-client-rapid-input-session').length`);
      await run(`window.__modUiClientFixture.captureRapidInput(); window.__modUiClientFixture.holdNextRapidRunRejection()`);
      await sendText('s');
      await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidRunRejectionHeld`);
      if (holdParentCommit) await run(`window.__holdRapidParentCommit = true`);
      await run(`window.__modUiClientFixture.invalidateRapidSite('after-stale-rejection')`);
      if (holdParentCommit) {
        await wait(`typeof window.__heldRapidParentCommit === 'function'`);
      } else {
        await wait(`window.__modUiClientFixture.calls.some(call => call.kind === 'operation'
          && call.sessionId === 'mod-ui-client-rapid-input-session' && call.request.type === 'draw_commit'
          && call.request.render_revision === ${beforeRejection.rapidRevision + 1} && call.returnedRevision === ${beforeRejection.rapidRevision + 1})`);
        await wait(`window.__modUiClientFixture.rapidInputSnapshot().inputReadOnly === true`);
      }
      await run(`window.__modUiClientFixture.releaseRapidRunRejection()`);
      await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidRunRejections === ${beforeRejection.rapidRunRejections + 1}`);
      await delay(80);
      assert.equal(await run(`window.__modUiClientFixture.calls.filter(call => call.kind === 'control'
        && call.request.subtype === 'ui_client_fault' && call.sessionId === 'mod-ui-client-rapid-input-session').length`), faultsBefore,
      `stale run rejection must not report a fault (parent commit held: ${holdParentCommit})`);
      assert.equal(await run(`Boolean(document.querySelector('[data-client="rapid-input"] .mod-ui-client-fallback'))`), false,
        `stale run rejection must not replace the Client surface (parent commit held: ${holdParentCommit})`);
      if (holdParentCommit) {
        await run(`(() => { window.__holdRapidParentCommit = false; const pendingCommit = window.__heldRapidParentCommit;
          window.__heldRapidParentCommit = null; pendingCommit(); })()`);
      }
      await wait(`window.__modUiClientFixture.rapidInputSnapshot().revisionFrameHeld`);
      await run(`window.__modUiClientFixture.releaseRapidRevisionFrame()`);
      await wait(`window.__modUiClientFixture.rapidInputSnapshot().inputReadOnly === false`);
      const afterRejection = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
      assert.equal(afterRejection.sameInputNode, true);
      assert.equal(afterRejection.value, 's');
      assert.match(afterRejection.unconfirmedNotice, /not confirmed/i);
      await retryUnconfirmed('Retry change');
      await wait(`window.__modUiClientFixture.rapidInputSnapshot().unconfirmedNotice === null`);
    }

    // A submit delayed across a parent revision must stay a submit, but only
    // the explicit Retry submit action may execute it on the new revision.
    await run(`window.__modUiClientFixture.emitRapidExternalValue('')`);
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.value === ''`);
    const beforeSubmit = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    await sendText('v');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidOnInputCallbacks.length === ${beforeSubmit.rapidOnInputCallbacks.length + 1}`);
    await run(`window.__modUiClientFixture.holdNextRapidControl()`);
    sendEnter();
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidControlHeld`);
    await run(`window.__modUiClientFixture.invalidateRapidSite('after-unconfirmed-submit')`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().inputReadOnly === true`);
    await run(`window.__modUiClientFixture.releaseRapidControl()`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().revisionFrameHeld`);
    await run(`window.__modUiClientFixture.releaseRapidRevisionFrame()`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().inputReadOnly === false`);
    const unconfirmedSubmit = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(unconfirmedSubmit.value, 'v');
    assert.equal(unconfirmedSubmit.retryLabel, 'Retry submit');
    assert.match(unconfirmedSubmit.unconfirmedNotice, /may already have run/i);
    assert.equal(unconfirmedSubmit.originalSubmitValue, '"v"');
    assert.deepEqual(unconfirmedSubmit.rapidOnSubmitCallbacks, beforeSubmit.rapidOnSubmitCallbacks);
    await assertUnconfirmedEnterBlocked();
    // Editing after an uncertain submit is a separate change, not confirmation
    // of that submit. In particular its ACK must not enable a fresh Enter.
    const testUnconfirmedEdit = process.env.LINGXI_TEST_SKIP_UNCONFIRMED_EDIT !== '1';
    const newerDraft = testUnconfirmedEdit ? 'vx' : 'v';
    const retryValue = 'v';
    if (testUnconfirmedEdit) {
      await sendText('x');
      await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot();
        return s.rapidOnInputCallbacks.length === ${beforeSubmit.rapidOnInputCallbacks.length + 2}
          && s.rapidPropValue === 'vx'; })()`);
      await delay(80);
      const afterEdit = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
      assert.equal(afterEdit.value, 'vx');
      assert.equal(afterEdit.retryLabel, 'Retry submit', 'a successful change must not confirm an earlier submit');
      assert.match(afterEdit.unconfirmedNotice, /may already have run/i);
      assert.match(afterEdit.unconfirmedNotice, /original submitted value/i);
      assert.equal(afterEdit.originalSubmitValue, '"v"', 'the retry notice must identify the original submitted value');
      await assertUnconfirmedEnterBlocked();
    }
    const beforeRetrySubmit = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    // A timed-out control response is not proof that the submit executed.
    await run(`window.__modUiClientFixture.timeoutNextRapidControl()`);
    await retryUnconfirmed('Retry submit');
    await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot();
      const button = document.querySelector('[data-client="rapid-input"] .mod-ui-client-input-retry');
      return s.rapidControlTimeouts === ${beforeRetrySubmit.rapidControlTimeouts + 1}
        && button?.getAttribute('aria-busy') !== 'true'; })()`);
    const timedOutRetry = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(timedOutRetry.retryLabel, 'Retry submit', 'a timeout must not confirm a submit');
    assert.equal(timedOutRetry.value, newerDraft);
    assert.deepEqual(timedOutRetry.rapidOnSubmitCallbacks, beforeRetrySubmit.rapidOnSubmitCallbacks);
    // A Host noop is not a successful callback. The warning must survive and
    // the user must be able to retry after the rejected attempt settles.
    await run(`window.__modUiClientFixture.ignoreNextRapidRunAsStale()`);
    await retryUnconfirmed('Retry submit');
    await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot();
      const button = document.querySelector('[data-client="rapid-input"] .mod-ui-client-input-retry');
      return s.runHeld.length === ${beforeRetrySubmit.runHeld.length + 1}
        && button?.getAttribute('aria-busy') !== 'true'; })()`);
    const rejectedRetry = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(rejectedRetry.retryLabel, 'Retry submit', 'a noop must not confirm a submit');
    assert.equal(rejectedRetry.value, newerDraft);
    assert.deepEqual(rejectedRetry.rapidOnSubmitCallbacks, beforeRetrySubmit.rapidOnSubmitCallbacks);
    const heldRetrySubmit = beforeRetrySubmit.rapidPostOrdinal + 1;
    await run(`window.__modUiClientFixture.holdRapidPostAt(${heldRetrySubmit})`);
    if (process.env.LINGXI_TEST_SKIP_OLD_ACK_RACE !== '1') {
      await run(`window.__modUiClientFixture.holdNextRapidRunResponse()`);
    }
    await retryUnconfirmed('Retry submit');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidBackgroundPending.some(p => p.ordinal === ${heldRetrySubmit})`);
    const retrySubmitPending = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.deepEqual(retrySubmitPending.rapidInputCallbacks.at(-1), { kind: 'submit', value: retryValue },
      'retrying an unconfirmed submit must call onSubmit');
    assert.equal(retrySubmitPending.rapidOnInputCallbacks.length, beforeSubmit.rapidOnInputCallbacks.length + (testUnconfirmedEdit ? 2 : 1));
    assert.equal(retrySubmitPending.rapidOnSubmitCallbacks.length, beforeSubmit.rapidOnSubmitCallbacks.length + 1);
    assert.equal(await run(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input-retry').getAttribute('aria-disabled')`), 'true');
    window.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'ENTER' });
    window.webContents.sendInputEvent({ type: 'char', keyCode: '\r' });
    window.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'ENTER' });
    await delay(50);
    assert.equal((await run(`window.__modUiClientFixture.rapidInputSnapshot()`)).rapidOnSubmitCallbacks.length,
      beforeSubmit.rapidOnSubmitCallbacks.length + 1, 'pending retry must not submit twice');
    await run(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input').focus()`);
    assert.equal((await run(`window.__modUiClientFixture.rapidInputSnapshot()`)).inputReadOnly, true);
    await sendText('z', false);
    const whileRetryPending = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(whileRetryPending.value, newerDraft, 'an in-flight retry must not lose a newer edit');
    assert.equal(whileRetryPending.controlRequests.length, retrySubmitPending.controlRequests.length);
    await run(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input-retry').focus()`);
    await run(`window.__modUiClientFixture.releaseRapidPostAt(${heldRetrySubmit})`);
    if (process.env.LINGXI_TEST_SKIP_OLD_ACK_RACE !== '1') {
      await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidRunResponseHeld`);
      // This independent frame has a higher sequence than the already-created
      // successful response, which remains in transport until released.
      await run(`window.__modUiClientFixture.emitRapidExternalValue('newer-before-submit-ack')`);
      await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidPropValue === 'newer-before-submit-ack'`);
      await run(`window.__modUiClientFixture.releaseRapidRunResponse()`);
    }
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().unconfirmedNotice === null`);
    const completedRetrySubmit = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(completedRetrySubmit.rapidOnSubmitCallbacks.length, beforeSubmit.rapidOnSubmitCallbacks.length + 1,
      'a second activation while retrying must not submit later');
    assert.equal(completedRetrySubmit.clientFocused, true,
      'a completed submit retry must return focus to the input');
    assert.equal(completedRetrySubmit.value,
      testUnconfirmedEdit ? newerDraft : process.env.LINGXI_TEST_SKIP_OLD_ACK_RACE === '1' ? retryValue : 'newer-before-submit-ack',
      'the latest frame must not erase a separate newer draft');
    milestone('uncertain submit and newer draft complete');

    // A submit can be older than a change already queued behind it. Losing
    // the older pending event at a revision boundary would offer Retry change
    // even though the submit may have run.
    await clearRapidDraft();
    const beforeQueuedChange = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    await sendText('n');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidOnInputCallbacks.length === ${beforeQueuedChange.rapidOnInputCallbacks.length + 1}`);
    await run(`window.__modUiClientFixture.holdNextRapidControl()`);
    sendEnter();
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidControlHeld`);
    await sendText('m');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().value === 'nm'`);
    await run(`window.__modUiClientFixture.invalidateRapidSite('after-queued-change')`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().retryLabel === 'Retry submit'`);
    await run(`window.__modUiClientFixture.releaseRapidControl()`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().revisionFrameHeld`);
    await run(`window.__modUiClientFixture.releaseRapidRevisionFrame()`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().inputReadOnly === false`);
    const afterQueuedChange = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(afterQueuedChange.value, 'nm');
    assert.equal(afterQueuedChange.retryLabel, 'Retry submit', 'a newer queued change must not hide an older uncertain submit');
    assert.equal(afterQueuedChange.originalSubmitValue, '"n"');
    assert.deepEqual(afterQueuedChange.rapidOnSubmitCallbacks, beforeQueuedChange.rapidOnSubmitCallbacks);
    await assertUnconfirmedEnterBlocked();
    await retryUnconfirmed('Retry submit');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().unconfirmedNotice === null`);
    const queuedChangeRetry = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.deepEqual(queuedChangeRetry.rapidInputCallbacks.at(-1), { kind: 'submit', value: 'n' });
    assert.equal(queuedChangeRetry.value, 'nm', 'retrying the original submission must preserve the newer draft');
    assert.equal(queuedChangeRetry.rapidOnSubmitCallbacks.length, beforeQueuedChange.rapidOnSubmitCallbacks.length + 1);

    // The original value is text, never HTML. A long value remains completely
    // available in a bounded scroll area associated with the retry button.
    await clearRapidDraft();
    const longOriginal = `<img src=x onerror=1>${'x'.repeat(4096)}`;
    await run(`window.__modUiClientFixture.emitRapidExternalValue(${JSON.stringify(longOriginal)})`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().value === ${JSON.stringify(longOriginal)}`);
    await run(`window.__modUiClientFixture.holdNextRapidControl()`);
    sendEnter();
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidControlHeld`);
    await run(`window.__modUiClientFixture.invalidateRapidSite('long-original')`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().retryLabel === 'Retry submit'`);
    await run(`window.__modUiClientFixture.releaseRapidControl()`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().revisionFrameHeld`);
    await run(`window.__modUiClientFixture.releaseRapidRevisionFrame()`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().inputReadOnly === false`);
    const originalPresentation = await run(`(() => {
      const code = document.querySelector('[data-client="rapid-input"] [data-submitted-value]');
      const button = document.querySelector('[data-client="rapid-input"] .mod-ui-client-input-retry');
      const status = document.getElementById(button?.getAttribute('aria-describedby'));
      return { text: code?.textContent, hasImage: Boolean(code?.querySelector('img')),
        scrollable: code?.scrollHeight > code?.clientHeight, keyboardFocusable: code?.tabIndex === 0,
        described: status?.contains(code) };
    })()`);
    assert.equal(originalPresentation.text, JSON.stringify(longOriginal));
    assert.equal(originalPresentation.hasImage, false, 'the submitted value must remain escaped text');
    assert.equal(originalPresentation.scrollable, true, 'long submitted values must not expand the dialog without bound');
    assert.equal(originalPresentation.keyboardFocusable, true, 'the original value must be scrollable with a keyboard');
    assert.equal(originalPresentation.described, true, 'the retry must describe the original submitted value');
    await retryUnconfirmed('Retry submit');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().unconfirmedNotice === null`);
    assert.deepEqual((await run(`window.__modUiClientFixture.rapidInputSnapshot()`)).rapidInputCallbacks.at(-1),
      { kind: 'submit', value: longOriginal });

    // Reorder two keyed ancestor Boxes while an Input callback is held.
    // The key, draft, focus and pending callback must stay with the Input.
    await clearRapidDraft();
    await run(`window.__modUiClientFixture.enableRapidGroups()`);
    await wait(`Boolean(document.querySelector('[data-client="rapid-input"] .mod-ui-client-surface > .mod-ui-client-box > .mod-ui-client-box .mod-ui-client-input'))`);
    await run(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input').focus(); window.__modUiClientFixture.captureRapidInput()`);
    const beforeReorder = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    const heldReorderPost = beforeReorder.rapidPostOrdinal + 1;
    await run(`window.__modUiClientFixture.holdRapidPostAt(${heldReorderPost})`);
    await sendText('k');
    await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot();
      return s.value === 'k' && s.rapidBackgroundPending.some(p => p.ordinal === ${heldReorderPost}); })()`);
    await run(`window.__modUiClientFixture.swapRapidGroups()`);
    await wait(`Boolean(document.querySelector('[data-client="rapid-input"] .mod-ui-client-surface > .mod-ui-client-box > .mod-ui-client-box:first-child .mod-ui-client-button'))`);
    const reordered = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(reordered.sameInputNode, true, 'a keyed ancestor reorder must keep the Input node');
    assert.equal(reordered.clientFocused, true, 'a keyed ancestor reorder must retain focus');
    assert.equal(reordered.value, 'k', 'a keyed ancestor reorder must retain the pending draft');
    await run(`window.__modUiClientFixture.releaseRapidPostAt(${heldReorderPost})`);
    await wait(`(() => { const s = window.__modUiClientFixture.rapidInputSnapshot();
      return !s.rapidBackgroundPending.some(p => p.ordinal === ${heldReorderPost}) && s.rapidPropValue === 'k'; })()`);
    assert.equal((await run(`window.__modUiClientFixture.rapidInputSnapshot()`)).sameInputNode, true);

    // Hold Native control while its owning ParentSite is replaced. Its late
    // response must not schedule runHeld even if passive cleanup is delayed.
    await run(`window.__modUiClientFixture.holdNextRapidControl()`);
    await sendText('m');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidControlHeld`);
    const beforeOwnerChange = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    await run(`window.__deferRapidPassiveCleanup = true; window.__modUiClientFixture.replaceRapidSite()`);
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.readOnly === false
      && window.__modUiClientFixture.calls.some(call => call.kind === 'operation' && call.request.type === 'draw_commit'
        && call.request.instance_id === 'rapid-input-site-replacement' && call.returnedRevision === ${beforeOwnerChange.rapidRevision})`);
    await run(`window.__modUiClientFixture.releaseRapidControl(); new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
    const afterOwnerChange = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(afterOwnerChange.runHeld.length, beforeOwnerChange.runHeld.length,
      'a late Native control result cannot start runHeld for a removed ParentSite');
    await run(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input').focus()`);

    // This time Native has reached the target and runHeld is waiting behind a
    // resize. Replacing the ParentSite must invalidate it before execution.
    const beforeQueuedOwnerChange = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    await run(`(() => { window.__modUiClientFixture.holdNextRapidResize();
      const surface = document.querySelector('[data-client="rapid-input"]');
      surface.style.width = Math.max(100, Math.floor(surface.getBoundingClientRect().width) - 60) + 'px'; })()`);
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().rapidResizeHeld`);
    await sendText('u');
    await wait(`window.__modUiClientFixture.rapidInputSnapshot().controlRequests.length === ${beforeQueuedOwnerChange.controlRequests.length + 1}`);
    await run(`new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
    await run(`window.__modUiClientFixture.replaceRapidSite('rapid-input-queued-replacement')`);
    await run(`window.__modUiClientFixture.releaseRapidResize()`);
    await wait(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.readOnly === false
      && window.__modUiClientFixture.calls.some(call => call.kind === 'operation' && call.request.type === 'draw_commit'
        && call.request.instance_id === 'rapid-input-queued-replacement' && call.returnedRevision === ${beforeQueuedOwnerChange.rapidRevision})`);
    await run(`new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
    const afterQueuedOwnerChange = await run(`window.__modUiClientFixture.rapidInputSnapshot()`);
    assert.equal(afterQueuedOwnerChange.runHeld.length, beforeQueuedOwnerChange.runHeld.length,
      'a queued callback must not run after its ParentSite is replaced');
    milestone('queued owner replacement complete');
    await run(`document.querySelector('[data-client="rapid-input"] .mod-ui-client-input').focus()`);

    // A newer sequence from a different live runtime must never be accepted
    // as the response to this Client's runHeld operation.
    await run(`window.__modUiClientFixture.enableRapidSibling()`);
    await wait(`document.querySelector('[data-client="rapid-sibling"]')?.textContent.includes('Rapid sibling alive')
      && document.querySelector('[data-client="rapid-input"] .mod-ui-client-input')?.readOnly === false`);
    await run(`window.__modUiClientFixture.misrouteNextRapidFrame()`);
    await sendText('x');
    await wait(`Boolean(document.querySelector('[data-client="rapid-input"] .mod-ui-client-fallback'))`);
    assert.equal(await run(`document.querySelector('[data-client="rapid-sibling"]')?.textContent.includes('Rapid sibling alive')`), true);
    assert.equal(await run(`window.__modUiClientFixture.calls.some(call => call.kind === 'operation'
      && call.request.type === 'runHeld' && call.request.runtimeId === 'runtime-rapid-input'
      && call.returnedRuntimeId === 'runtime-rapid-sibling')`), true);
    assert.equal(await run(`window.__modUiClientFixture.calls.some(call => call.kind === 'control'
      && call.request.subtype === 'ui_client_fault' && call.request.client === 'rapid-input'
      && call.request.reason.includes('different runtime'))`), true);

    process.stdout.write(`${JSON.stringify({ initialStatusMountSuperseded, initialSiblingMountSuperseded, parentElementsRendered, parentButtonRequest, parentInputChangeRequest, parentInputSubmitRequest, parentSelectRequest, parentMarkdownRequest, parentControlRequestsMatchNativeBodies, clientIdentitySnapshot, clientIdentityPropagationMatchesAttach, buttonPressHandled, buttonUsesCurrentHandle, inputUsesHookReachedValue, inputSubmitUsesHookReachedValue, inputRunHeldEvents, selectUsesHookReachedValue, selectRunHeldEvents, markdownExternalLinkUsesNormalNavigation, markdownOpenLinkUsesNormalNavigation, markdownLinksDoNotDispatchClientCallbacks, contextMentionHref, contextMentionClickPreventedNavigation, contextMentionOpenEvents, ordinaryMentionClickPreventedNavigation, ordinaryMentionOpenEvents, preRegistrationFrameReplayed, staleOperationIgnoredWithoutFault, parentPropsUpdateRendered, staleRpcFrameIgnored, staleFrameIgnored, workerFaultRenderedWithoutDuplicateReport, workerFaultDiagnostics, workerFaultLeavesSiblingVisible, workerFaultNotClearedByParentRevision, workerFaultRecoveredFromEnvironmentRemount, asyncWorkerFaultRenderedWithoutDuplicateReport, bufferedAsyncWorkerFaultIsLeafLocal, statusReadyAfterAsyncClientRemoval, statusReadinessTimeline, adapterFaultReportedAsRun, faultLeavesSiblingVisible, adapterRunFaultSurvivedPassiveFrames, adapterRunFaultSurvivedNewRevisionSameIdentity, adapterRunFaultSurvivedSameIdentityPropsUpdate, adapterRunFaultRecoveredAfterStateTokenChange, adapterRunFaultRecoveredAfterClientRemount, pluginUnloadInvalidationRendered, emptyNoHostIsQuietAndCleansUp, runtimeLifecycleSnapshot, unmountReleasedRuntimeAndDrawIdentity, staleRenderCleanupPreservedNewerFrame, lateInitialRenderCleanedUp, rapidBeforeRelease: rapidBeforeRelease, rapidAfterFirstAck, rapidInputRegression, focusDiagnostics: { beforeActivation: documentFocusedBeforeActivation, afterActivation: documentFocusedAfterActivation } })}\n`);
  } finally {
    window.destroy();
    app.quit();
  }
}

main().catch((error) => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
