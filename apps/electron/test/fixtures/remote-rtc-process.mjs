/** Observe a real fixture process. Failure to launch or reach READY is fatal. */
export function waitForRtcFixture(child, { startupTimeoutMs = 40_000, fixtureTimeoutMs = 160_000 } = {}) {
  if (!child.stdout || !child.stderr) throw new Error('RTC fixture requires stdout and stderr pipes');

  return new Promise((resolve, reject) => {
    let stdout = '';
    let stderr = '';
    let lines = '';
    let ready = false;
    let settled = false;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(startTimer);
      clearTimeout(fixtureTimer);
      if (error) reject(error);
      else resolve(value);
    };
    const diagnostic = () => `\nStderr: ${stderr}\nOutput: ${stdout}`;
    const startTimer = setTimeout(() => {
      if (!ready) {
        child.kill();
        finish(new Error(`Electron did not report READY within ${startupTimeoutMs} ms${diagnostic()}`));
      }
    }, startupTimeoutMs);
    const fixtureTimer = setTimeout(() => {
      child.kill();
      finish(new Error(`Electron RTC fixture timed out after ${fixtureTimeoutMs} ms${diagnostic()}`));
    }, fixtureTimeoutMs);

    child.stdout.setEncoding('utf8').on('data', (chunk) => {
      stdout += chunk;
      lines += chunk;
      for (;;) {
        const end = lines.indexOf('\n');
        if (end < 0) break;
        const line = lines.slice(0, end).trim();
        lines = lines.slice(end + 1);
        if (line === 'READY') {
          ready = true;
          clearTimeout(startTimer);
          continue;
        }
        if (!line.startsWith('{')) continue;
        let result;
        try { result = JSON.parse(line); } catch { continue; }
        if (result.passed === false) {
          finish(new Error(`Fixture failed: ${result.error}${diagnostic()}`));
        } else if (result.passed === true) {
          if (!ready) finish(new Error(`Fixture reported success before READY${diagnostic()}`));
          else finish(null, result);
        }
      }
    });
    child.stderr.setEncoding('utf8').on('data', (chunk) => { stderr += chunk; });
    child.once('error', (error) => finish(new Error(`Electron could not be launched: ${error.message}`, { cause: error })));
    // close, unlike exit, waits for the stdout/stderr streams to deliver any
    // final READY or result line. The timers still bound a lingering pipe.
    child.once('close', (code, signal) => {
      const status = code ?? signal;
      if (!ready) finish(new Error(`Electron exited ${status} before READY${diagnostic()}`));
      else finish(new Error(`Electron exited ${status} without a passing result${diagnostic()}`));
    });
  });
}
