import { app, BrowserWindow } from 'electron';

const url = process.argv.find((argument) => argument.startsWith('http://') || argument.startsWith('https://'));
if (!url) throw new Error('fixture URL is required');

const testUserData = process.env.LINGXI_TEST_USER_DATA;
if (testUserData) app.setPath('userData', testUserData);
app.commandLine.appendSwitch('disable-gpu');
app.commandLine.appendSwitch('disable-software-rasterizer');

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
async function waitFor(webContents, expression, timeout = 6000) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    last = await webContents.executeJavaScript(expression);
    if (last) return last;
    await delay(25);
  }
  throw new Error(`timed out waiting for: ${expression} (last value ${JSON.stringify(last)})`);
}

/** Emits one audio_request and waits for the renderer to answer it. */
async function roundTrip(webContents, requestId, op) {
  const before = await webContents.executeJavaScript('window.__audioRequestTest.commands().length');
  await webContents.executeJavaScript(
    `window.__audioRequestTest.emit(${requestId}, ${JSON.stringify(op)})`,
  );
  await waitFor(webContents, `window.__audioRequestTest.commands().length > ${before}`);
  // Settle: if the renderer were to answer a second time, this window catches it.
  await delay(150);
  return webContents.executeJavaScript(
    `window.__audioRequestTest.commands().slice(${before}).map((entry) => entry)`,
  );
}

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 900, height: 700, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const { webContents } = window;
    await waitFor(webContents, 'Boolean(window.__audioRequestTest)');
    // The effect has run and exactly one subscription survived StrictMode's
    // mount/unmount/mount cycle — a leaked one would answer twice.
    const listenerCount = await webContents.executeJavaScript('window.__audioRequestTest.listenerCount()');

    const isRecording = await roundTrip(webContents, 41, { type: 'is_recording' });
    const transcribe = await roundTrip(webContents, 42, { type: 'transcribe', language: 'en-US' });

    // A response the engine no longer wants must be inert, not fatal.
    await webContents.executeJavaScript('window.__audioRequestTest.rejectRequestId(999)');
    const dropped = await roundTrip(webContents, 999, { type: 'is_recording' });
    await delay(200);
    const crashed = await webContents.executeJavaScript('window.__audioRequestTest.crashed()');
    const stillAlive = await webContents.executeJavaScript(
      'Boolean(document.getElementById("ready")) && Boolean(window.__audioRequestTest)',
    );

    process.stdout.write(`${JSON.stringify({
      listenerCount,
      isRecording,
      transcribe,
      dropped,
      crashed,
      stillAlive,
    })}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
  }
}

// `app.exit` rather than `app.quit` on BOTH paths: when this fixture is used
// as a negative control (delete the subscription in `useBridge` and watch this
// go red), `await app.quit()` inside a `finally` does not reliably resolve
// while the app is already tearing down, and the child would hang until the
// test's own 30s timeout instead of reporting the real error. A negative
// control that takes 30s to say "timed out" is a much worse signal than one
// that exits in seconds with the failing expression.
main().then(
  () => { app.exit(0); },
  (error) => {
    process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`);
    app.exit(1);
  },
);
