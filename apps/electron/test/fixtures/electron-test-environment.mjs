// Hidden fixtures need a frame clock and focus independent of CI host settings.
export async function configureFixtureWindow(window) {
  const contents = window.webContents;
  contents.setBackgroundThrottling(false);
  if (!contents.debugger.isAttached()) contents.debugger.attach('1.3');
  await contents.debugger.sendCommand('Emulation.setFocusEmulationEnabled', { enabled: true });
  await contents.debugger.sendCommand('Emulation.setEmulatedMedia', {
    features: [{ name: 'prefers-reduced-motion', value: 'no-preference' }],
  });
}

export async function waitForFixture(contents, expression, timeout = 8000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await contents.executeJavaScript(expression)) return;
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  throw new Error('Fixture condition timed out: ' + expression);
}

export async function settleFixtureAnimations(contents, selector) {
  // React starts entrance animations in a following frame. Let them start,
  // then require finite animations to finish before measuring geometry.
  await contents.executeJavaScript('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  await waitForFixture(contents, '(() => { const element = document.querySelector(' + JSON.stringify(selector) + '); return element && element.getAnimations({ subtree: true }).every(animation => animation.effect?.getComputedTiming().iterations === Infinity || animation.playState === "finished" || animation.playState === "idle"); })()');
}
