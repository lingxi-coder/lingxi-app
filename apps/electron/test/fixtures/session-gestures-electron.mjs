import { configureFixtureWindow } from './electron-test-environment.mjs';
import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';

const url = process.argv.find(argument => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1000, height: 800, webPreferences: { sandbox: true, backgroundThrottling: false } });
  let inputPhase = 'startup';
  let lastSentMouse = null;
  const inputTrace = [];
  const traceInput = (kind, details = {}) => {
    inputTrace.push({ at: Date.now(), phase: inputPhase, kind, focused: window.isFocused(),
      ...(process.platform === 'darwin' ? { appActive: app.isActive() } : {}), ...details });
    if (inputTrace.length > 120) inputTrace.shift();
  };
  window.on('focus', () => traceInput('window-focus'));
  window.on('blur', () => traceInput('window-blur'));
  window.webContents.on('before-mouse-event', (_event, mouse) => traceInput('before-mouse-event', {
    type: mouse.type, x: mouse.x, y: mouse.y, globalX: mouse.globalX, globalY: mouse.globalY,
    button: mouse.button, clickCount: mouse.clickCount,
  }));
  window.webContents.on('input-event', (_event, input) => traceInput('webContents-input-event', {
    type: input.type, x: input.x, y: input.y, key: input.key, button: input.button,
  }));
  try {
    await window.loadURL(url);
    await configureFixtureWindow(window);
    const run = script => window.webContents.executeJavaScript(script);
    await run(`(() => {
      window.sessionGesturesInputTrace = [];
      for (const type of ['pointerdown', 'pointerup', 'pointermove', 'pointercancel', 'pointerover', 'pointerout',
        'mousedown', 'mouseup', 'mousemove', 'mouseover', 'mouseout', 'click', 'contextmenu']) {
        document.addEventListener(type, event => {
          const target = event.target instanceof Element ? event.target : null;
          const session = target?.closest('[data-session-id]');
          const menu = target?.closest('[role="menu"]');
          const trace = window.sessionGesturesInputTrace;
          trace.push({ at: Date.now(), type, trusted: event.isTrusted, targetTag: target?.tagName,
            targetClass: typeof target?.className === 'string' ? target.className : '',
            sessionId: session?.dataset.sessionId, menuLabel: menu?.getAttribute('aria-label'),
            clientX: event.clientX, clientY: event.clientY, pointerId: event.pointerId,
            button: event.button, buttons: event.buttons, documentHasFocus: document.hasFocus() });
          if (trace.length > 120) trace.shift();
        }, true);
      }
    })()`);
    const diagnosticSnapshot = async () => {
      let renderer;
      try {
        renderer = await run(`(() => {
          const lastSentMouse = ${JSON.stringify(lastSentMouse)};
          const rect = element => {
            if (!element) return null;
            const bounds = element.getBoundingClientRect();
            return { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height };
          };
          const rows = [...document.querySelectorAll('.sidebar-tree-row')].map(row => {
            const button = row.querySelector('[data-session-id]');
            return { sessionId: button?.dataset.sessionId, title: button?.title,
              rowBounds: rect(row), buttonBounds: rect(button), rowHovered: row.matches(':hover'),
              buttonHovered: button?.matches(':hover'), dragging: row.dataset.dragging,
              dragTarget: row.dataset.dragTarget };
          });
          const hit = lastSentMouse ? document.elementFromPoint(lastSentMouse.x, lastSentMouse.y) : null;
          return {
            documentHasFocus: document.hasFocus(),
            layout: ['.desktop-navigation', '.desktop-body', '.desktop-surface', '.desktop-sidebar', '.desktop-sidebar-list'].map(selector => {
              const element = document.querySelector(selector);
              const style = element && getComputedStyle(element);
              return { selector, bounds: rect(element), overflow: style?.overflow, pointerEvents: style?.pointerEvents,
                visibility: style?.visibility, opacity: style?.opacity, zIndex: style?.zIndex, inert: element?.inert,
                backdropFilter: style?.backdropFilter, transform: style?.transform };
            }),
            hitStack: lastSentMouse && document.elementsFromPoint(lastSentMouse.x, lastSentMouse.y).map(element => ({
              tag: element.tagName, className: typeof element.className === 'string' ? element.className : '',
            })),
            activeElement: document.activeElement && { tag: document.activeElement.tagName,
              ariaLabel: document.activeElement.getAttribute('aria-label'),
              sessionId: document.activeElement.dataset?.sessionId },
            lastSentMouse, hitTarget: hit && { tag: hit.tagName, className: typeof hit.className === 'string' ? hit.className : '',
              sessionId: hit.closest?.('[data-session-id]')?.dataset.sessionId,
              menuLabel: hit.closest?.('[role="menu"]')?.getAttribute('aria-label') },
            rows,
            menus: [...document.querySelectorAll('[role="menu"]')].map(menu => ({
              role: menu.getAttribute('role'), ariaLabel: menu.getAttribute('aria-label'),
              className: menu.className, text: menu.innerText, bounds: rect(menu) })),
            pointerEvents: window.sessionGesturesInputTrace.slice(-100),
          };
        })()`);
      } catch (error) {
        renderer = { snapshotError: String(error) };
      }
      return {
        phase: inputPhase,
        browserWindowFocused: window.isFocused(),
        ...(process.platform === 'darwin' ? { appActive: app.isActive() } : {}),
        mainInputEvents: inputTrace.slice(-100),
        renderer,
      };
    };
    const focusForInput = async phase => {
      inputPhase = phase;
      traceInput('focus-request');
      // configureFixtureWindow supplies Chromium focus without sharing the
      // system pointer, whose movements can cancel a hold or drag mid-test.
      await wait('document.hasFocus()');
      traceInput('emulated-focus-confirmed');
    };
    const wait = async expression => {
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) { if (await run(expression)) return; await delay(20); }
      const snapshot = await diagnosticSnapshot();
      throw new Error(`Timed out: ${expression}\ndiagnostics=${JSON.stringify(snapshot)}\n${await run('document.body.innerText')}`);
    };
    const selector = id => `document.querySelector('[data-session-id="${id}"]')`;
    const point = id => run(`(() => { const r = ${selector(id)}.getBoundingClientRect(); return { x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) }; })()`);
    const hitTargetExpression = id => `(() => {
      const button = ${selector(id)};
      if (!button) return false;
      const rect = button.getBoundingClientRect();
      const hit = document.elementFromPoint(Math.round(rect.x + rect.width / 2), Math.round(rect.y + rect.height / 2));
      return hit?.closest('[data-session-id]') === button;
    })()`;
    const input = async (type, p, extra = {}) => {
      const event = { type, ...p, ...extra };
      if (type.startsWith('mouse') && Number.isFinite(event.x) && Number.isFinite(event.y)) lastSentMouse = { x: event.x, y: event.y, type };
      traceInput('sendInputEvent', { type, x: event.x, y: event.y, button: event.button, modifiers: event.modifiers });
      window.webContents.sendInputEvent(event);
      await delay(40);
    };
    const down = p => input('mouseDown', p, { button: 'left', clickCount: 1 });
    const up = p => input('mouseUp', p, { button: 'left', clickCount: 1 });
    const click = async id => { await wait(hitTargetExpression(id)); const p = await point(id); await input('mouseMove', p); await down(p); await up(p); };
    const opened = () => run('window.sessionGesturesFixture.opened');
    await wait(`Boolean(${selector('gamma')})`);
    await wait(`document.querySelector('.desktop-sidebar')?.inert === false`);
    await focusForInput('alpha hover, focus, and click');
    await delay(150);
    let alpha = await point('alpha');
    await input('mouseMove', { x: 600, y: 600 });
    await input('mouseMove', alpha);
    const actionsVisibleExpression = id => `(() => {
      const row = ${selector(id)}.closest('.sidebar-tree-row');
      return ['Pin', 'Archive'].every(label => [...row.querySelectorAll('button')].some(button => {
        const name = button.getAttribute('aria-label') || button.title;
        const style = getComputedStyle(button);
        return name?.startsWith(label) && style.visibility !== 'hidden' && Number(style.opacity) > 0 && style.pointerEvents !== 'none';
      }));
    })()`;
    const actionsVisible = () => run(actionsVisibleExpression('alpha'));
    await wait(actionsVisibleExpression('alpha'));
    assert.equal(await actionsVisible(), true, 'hover exposes pin and archive');
    await input('mouseMove', { x: 600, y: 600 });
    await run(`${selector('alpha')}.focus()`);
    await wait(actionsVisibleExpression('alpha'));
    assert.equal(await actionsVisible(), true, 'keyboard focus exposes pin and archive');
    await click('alpha');
    assert.deepEqual(await opened(), ['alpha'], 'short click opens exactly once');
    await delay(200);
    const beta = await point('beta');
    await focusForInput('beta long press');
    await delay(80);
    await wait(hitTargetExpression('beta'));
    await input('mouseMove', beta);
    await delay(200);
    await down(beta);
    await delay(650);
    await wait(`Boolean(document.querySelector('[role="menu"][aria-label="Actions for beta"]'))`);
    await up(beta);
    assert.deepEqual(await opened(), ['alpha'], 'long press does not open the session');
    await focusForInput('dismiss beta action menu');
    await input('keyDown', {}, { keyCode: 'Escape' });
    await input('keyUp', {}, { keyCode: 'Escape' });
    await wait(`!document.querySelector('[role="menu"]')`);
    await focusForInput('gamma drag and click');
    await delay(80);
    await input('mouseMove', { x: 600, y: 600 });
    await up(beta);
    await wait(`!document.querySelector('[role="menu"]')`);
    await run(`${selector('gamma')}.scrollIntoView({ block: 'center', inline: 'nearest' })`);
    const visibleHitTargetExpression = id => `(() => {
      const button = ${selector(id)};
      if (!button) return false;
      const rect = button.getBoundingClientRect();
      if (rect.top < 0 || rect.bottom > window.innerHeight) return false;
      const hit = document.elementFromPoint(Math.round(rect.x + rect.width / 2), Math.round(rect.y + rect.height / 2));
      return hit?.closest('[data-session-id]') === button;
    })()`;
    await wait(visibleHitTargetExpression('gamma'));
    alpha = await point('alpha');
    const gamma = await point('gamma');
    const projectSessionOrder = `[...document.querySelectorAll('[data-session-project-path="/fixture"]')].map(button => button.dataset.sessionId)`;
    assert.deepEqual(await run(projectSessionOrder), ['alpha', 'beta', 'gamma'], 'drag starts in updated order');
    await input('mouseMove', gamma);
    await wait(hitTargetExpression('gamma'));
    await down(gamma);
    await input('mouseMove', { x: gamma.x + 12, y: gamma.y }, { button: 'left', modifiers: ['leftButtonDown'] });
    await wait(`${selector('gamma')}.closest('.sidebar-tree-row')?.dataset.dragging === 'true'`);
    // The feedback marks the source during initialization; the sensor accepts
    // moves only after the renderer completes that phase. Advance rendered
    // frames before sending the destination move instead of racing activation.
    await run('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
    const alphaHit = await run(`(() => {
      const element = document.elementFromPoint(${alpha.x}, ${alpha.y});
      const row = element?.closest('.sidebar-tree-row');
      const button = row?.querySelector('[data-session-id]');
      return { tag: element?.tagName, className: typeof element?.className === 'string' ? element.className : '', sessionId: button?.dataset.sessionId, projectPath: button?.dataset.sessionProjectPath };
    })()`);
    assert.equal(alphaHit.sessionId, 'alpha', `drag pointer did not resolve to alpha: ${JSON.stringify(alphaHit)}`);
    await input('mouseMove', alpha, { button: 'left', modifiers: ['leftButtonDown'] });
    // The source is already a drop target on activation. Wait for the actual
    // sortable preview to reach alpha before releasing; otherwise a delayed
    // mouseMove can arrive together with mouseUp and drop on the source.
    await wait(`JSON.stringify(${projectSessionOrder}) === JSON.stringify(['gamma', 'alpha', 'beta']) && ${selector('gamma')}.closest('.sidebar-tree-row')?.dataset.dragTarget === 'true'`);
    const dropIndicators = await run(`Array.from(document.querySelectorAll('.sidebar-tree-row[data-drag-target="true"] [data-session-id]')).map(button => button.dataset.sessionId)`);
    assert.deepEqual(dropIndicators, ['gamma'], 'the sortable placeholder marks the dragged item at its preview position');
    await up(alpha);
    await wait('window.sessionGesturesFixture.touched.length === 1');
    assert.deepEqual(await run('window.sessionGesturesFixture.touched'), ['gamma'], 'updated-sort drag touches the dragged session');
    await wait('window.sessionGesturesFixture.preferences.length === 1');
    const preferences = await run('window.sessionGesturesFixture.preferences[0]');
    assert.equal(preferences.chatSort, 'manual', 'drag automatically selects manual sorting');
    assert.deepEqual(preferences.manualSessionOrder['/fixture'], ['gamma', 'alpha', 'beta']);
    assert.deepEqual(await run(projectSessionOrder), ['gamma', 'alpha', 'beta'], 'DOM follows saved order');
    await wait(`typeof window.sessionGesturesFixture.finishTouch === 'function'`);
    await run(`window.sessionGesturesFixture.finishTouch()`);
    await wait(`window.sessionGesturesFixture.sessions.find(session => session.uuid === 'gamma')?.modified_rfc3339 === '2026-09-23T00:00:00Z'`);
    assert.equal(await run(`window.sessionGesturesFixture.sessions.find(session => session.uuid === 'gamma').modified_rfc3339`), '2026-09-23T00:00:00Z', 'drag updates session timestamp');
    assert.deepEqual(await opened(), ['alpha'], 'drag does not open a session');
    await click('gamma');
    assert.deepEqual(await opened(), ['alpha', 'gamma'], 'click works immediately after drag');
    await focusForInput('gamma same-row drag');
    // The first drag animates rows into their new positions. Their visible
    // bounds can overlap until the sortable animations finish, so a new
    // center-point press may otherwise hit a different row.
    await run('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
    await wait(`Array.from(document.querySelectorAll('.sidebar-session-row[data-session-reorderable="true"]')).every(row => row.getAnimations().every(animation => animation.playState === 'finished' || animation.playState === 'idle'))`);
    await wait(hitTargetExpression('gamma'));
    const currentGamma = await point('gamma');
    await input('mouseMove', currentGamma);
    await run(`window.fixtureSameRowDown = null; document.addEventListener('pointerdown', event => {
      window.fixtureSameRowDown = { trusted: event.isTrusted, sessionId: event.target instanceof Element ? event.target.closest('[data-session-id]')?.dataset.sessionId ?? null : null };
    }, { capture: true, once: true })`);
    await down(currentGamma);
    await wait('Boolean(window.fixtureSameRowDown)');
    assert.deepEqual(await run('window.fixtureSameRowDown'), { trusted: true, sessionId: 'gamma' }, 'same-row drag starts on gamma');
    await input('mouseMove', { x: currentGamma.x + 12, y: currentGamma.y }, { button: 'left', modifiers: ['leftButtonDown'] });
    await wait(`${selector('gamma')}.closest('.sidebar-tree-row')?.dataset.dragging === 'true' && ${selector('gamma')}.closest('.sidebar-tree-row')?.dataset.dragTarget === 'true' && JSON.stringify(${projectSessionOrder}) === JSON.stringify(['gamma', 'alpha', 'beta'])`);
    await up({ x: currentGamma.x + 12, y: currentGamma.y });
    await wait(`!document.querySelector('.sidebar-session-row[data-dragging="true"]')`);
    await run('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
    assert.equal(await run('window.sessionGesturesFixture.preferences.length'), 1, 'same-row drag does not persist an order');
    assert.deepEqual(await run('window.sessionGesturesFixture.touched'), ['gamma'], 'same-row drag does not touch the session');
    assert.deepEqual(await run(projectSessionOrder), ['gamma', 'alpha', 'beta'], 'same-row drag keeps the saved order');
    await click('gamma');
    assert.deepEqual(await opened(), ['alpha', 'gamma', 'gamma']);
    await focusForInput('pinned session long press');
    const pinned = await point('pinned');
    await input('mouseMove', pinned);
    await down(pinned);
    await delay(650);
    await wait(`Boolean(document.querySelector('[role="menu"][aria-label="Actions for pinned"]'))`);
    await up(pinned);
    assert.deepEqual(await opened(), ['alpha', 'gamma', 'gamma'], 'pinned session hold does not navigate');
    await focusForInput('dismiss pinned action menu');
    await input('keyDown', {}, { keyCode: 'Escape' });
    await input('keyUp', {}, { keyCode: 'Escape' });
    await wait(`!document.querySelector('[role="menu"]')`);
    // Cancellation has no mouse sendInputEvent equivalent. Deliver it to the actual
    // captured pointer after a native mouse down, then release outside the row.
    await focusForInput('cancel gamma pointer gesture');
    await run(`${selector('gamma')}.addEventListener('pointerdown', event => { window.fixturePointerId = event.pointerId; }, { once: true })`);
    const cancelPoint = await point('gamma');
    await input('mouseMove', cancelPoint);
    await down(cancelPoint);
    assert.equal(await run(`${selector('gamma')}.hasPointerCapture(window.fixturePointerId)`), true, 'native pointer down captures the actual pointer');
    await input('mouseMove', { x: cancelPoint.x + 12, y: cancelPoint.y }, { button: 'left', modifiers: ['leftButtonDown'] });
    await run(`${selector('gamma')}.dispatchEvent(new PointerEvent('pointercancel', { bubbles: true, pointerId: window.fixturePointerId }))`);
    await input('mouseMove', { x: 600, y: 600 }, { button: 'left', modifiers: ['leftButtonDown'] });
    await up({ x: 600, y: 600 });
    await delay(650);
    assert.equal(await run(`Boolean(document.querySelector('[data-dragging="true"], [role="menu"]'))`), false, 'cancel clears drag and hold');
    assert.equal(await run('window.sessionGesturesFixture.preferences.length'), 1, 'cancel does not save order');
    const opensBeforeClick = (await opened()).length;
    await focusForInput('click after cancellation');
    await click('gamma');
    assert.equal((await opened()).length, opensBeforeClick + 1, 'new click works after cancellation');
    // Optimistic sorting moves the live DOM during dragover. Recreate the
    // sidebar from the saved preferences with a newer alpha, so neither that
    // preview nor the updated timestamp order can satisfy this assertion.
    await run(`window.gestureBeforeRemount = ${selector('gamma')}; window.sessionGesturesFixture.remountWithNewerAlpha()`);
    await wait(`window.sessionGesturesFixture.sessions.find(session => session.uuid === 'alpha')?.modified_rfc3339 === '2026-09-24T00:00:00Z' && Boolean(${selector('gamma')}) && ${selector('gamma')} !== window.gestureBeforeRemount`);
    assert.deepEqual(await run(projectSessionOrder), ['gamma', 'alpha', 'beta'], 'manual order survives a sidebar remount when updated order differs');
    process.stdout.write(`${JSON.stringify({ passed: true })}\n`);
  } finally { window.destroy(); app.quit(); }
}
main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
