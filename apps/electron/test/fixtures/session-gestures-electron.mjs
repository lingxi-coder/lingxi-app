import { configureFixtureWindow } from './electron-test-environment.mjs';
import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';

const url = process.argv.find(argument => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: true, width: 1000, height: 800, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    await window.loadURL(url);
    await configureFixtureWindow(window);
    const run = script => window.webContents.executeJavaScript(script);
    const wait = async expression => {
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) { if (await run(expression)) return; await delay(20); }
      const rows = await run(`Array.from(document.querySelectorAll('.sidebar-tree-row')).map(row => ({
        sessionId: row.querySelector('[data-session-id]')?.dataset.sessionId,
        dragging: row.dataset.dragging,
        dragTarget: row.dataset.dragTarget,
      }))`);
      throw new Error(`Timed out: ${expression}\nrows=${JSON.stringify(rows)}\n${await run('document.body.innerText')}`);
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
    const input = async (type, p, extra = {}) => { window.webContents.sendInputEvent({ type, ...p, ...extra }); await delay(40); };
    const down = p => input('mouseDown', p, { button: 'left', clickCount: 1 });
    const up = p => input('mouseUp', p, { button: 'left', clickCount: 1 });
    const click = async id => { await wait(hitTargetExpression(id)); const p = await point(id); await input('mouseMove', p); await down(p); await up(p); };
    const opened = () => run('window.sessionGesturesFixture.opened');
    await wait(`Boolean(${selector('gamma')})`);
    window.focus();
    await delay(150);
    const alpha = await point('alpha');
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
    window.focus();
    await delay(80);
    await wait(hitTargetExpression('beta'));
    await input('mouseMove', beta);
    await delay(200);
    await down(beta);
    await delay(650);
    await wait(`Boolean(document.querySelector('[role="menu"][aria-label="Actions for beta"]'))`);
    await up(beta);
    assert.deepEqual(await opened(), ['alpha'], 'long press does not open the session');
    await input('keyDown', {}, { keyCode: 'Escape' });
    await input('keyUp', {}, { keyCode: 'Escape' });
    await wait(`!document.querySelector('[role="menu"]')`);
    window.focus();
    await delay(80);
    await input('mouseMove', { x: 600, y: 600 });
    await up(beta);
    await wait(`!document.querySelector('[role="menu"]')`);
    const gamma = await point('gamma');
    await input('mouseMove', gamma);
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
    await wait(`Array.from(document.querySelectorAll('.sidebar-tree-row')).some(row => row.dataset.dragTarget === 'true')`);
    const dropIndicators = await run(`Array.from(document.querySelectorAll('.sidebar-tree-row[data-drag-target="true"] [data-session-id]')).map(button => button.dataset.sessionId)`);
    assert.deepEqual(dropIndicators, ['gamma'], 'the sortable placeholder marks the dragged item at its preview position');
    await up(alpha);
    await wait('window.sessionGesturesFixture.touched.length === 1');
    assert.deepEqual(await run('window.sessionGesturesFixture.touched'), ['gamma'], 'updated-sort drag touches the dragged session');
    await wait('window.sessionGesturesFixture.preferences.length === 1');
    const preferences = await run('window.sessionGesturesFixture.preferences[0]');
    assert.equal(preferences.chatSort, 'manual', 'drag automatically selects manual sorting');
    assert.deepEqual(preferences.manualSessionOrder['/fixture'], ['gamma', 'alpha', 'beta']);
    assert.deepEqual(await run(`[...document.querySelectorAll('[data-session-project-path="/fixture"]')].map(button => button.dataset.sessionId)`), ['gamma', 'alpha', 'beta'], 'DOM follows saved order');
    assert.equal(await run(`window.sessionGesturesFixture.sessions.find(session => session.uuid === 'gamma').modified_rfc3339`), '2026-09-23T00:00:00Z', 'drag updates session timestamp');
    assert.deepEqual(await opened(), ['alpha'], 'drag does not open a session');
    await click('gamma');
    assert.deepEqual(await opened(), ['alpha', 'gamma'], 'click works immediately after drag');
    const currentGamma = await point('gamma');
    await down(currentGamma);
    await input('mouseMove', { x: currentGamma.x + 12, y: currentGamma.y }, { button: 'left', modifiers: ['leftButtonDown'] });
    await up({ x: currentGamma.x + 12, y: currentGamma.y });
    assert.equal(await run('window.sessionGesturesFixture.preferences.length'), 1, 'same-row drag does not persist an order');
    await click('gamma');
    assert.deepEqual(await opened(), ['alpha', 'gamma', 'gamma']);
    const pinned = await point('pinned');
    await input('mouseMove', pinned);
    await down(pinned);
    await delay(650);
    await wait(`Boolean(document.querySelector('[role="menu"][aria-label="Actions for pinned"]'))`);
    await up(pinned);
    assert.deepEqual(await opened(), ['alpha', 'gamma', 'gamma'], 'pinned session hold does not navigate');
    await input('keyDown', {}, { keyCode: 'Escape' });
    await input('keyUp', {}, { keyCode: 'Escape' });
    await wait(`!document.querySelector('[role="menu"]')`);
    // Cancellation has no mouse sendInputEvent equivalent. Deliver it to the actual
    // captured pointer after a native mouse down, then release outside the row.
    await run(`${selector('gamma')}.addEventListener('pointerdown', event => { window.fixturePointerId = event.pointerId; }, { once: true })`);
    const cancelPoint = await point('gamma');
    await input('mouseMove', cancelPoint);
    await down(cancelPoint);
    await input('mouseMove', { x: cancelPoint.x + 12, y: cancelPoint.y }, { button: 'left', modifiers: ['leftButtonDown'] });
    await run(`${selector('gamma')}.dispatchEvent(new PointerEvent('pointercancel', { bubbles: true, pointerId: window.fixturePointerId }))`);
    await input('mouseMove', { x: 600, y: 600 }, { button: 'left', modifiers: ['leftButtonDown'] });
    await up({ x: 600, y: 600 });
    await delay(650);
    assert.equal(await run(`Boolean(document.querySelector('[data-dragging="true"], [role="menu"]'))`), false, 'cancel clears drag and hold');
    assert.equal(await run('window.sessionGesturesFixture.preferences.length'), 1, 'cancel does not save order');
    const opensBeforeClick = (await opened()).length;
    await click('gamma');
    assert.equal((await opened()).length, opensBeforeClick + 1, 'new click works after cancellation');
    process.stdout.write(`${JSON.stringify({ passed: true })}\n`);
  } finally { window.destroy(); app.quit(); }
}
main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
