import { createInterface } from 'node:readline';
import { existsSync, writeFileSync } from 'node:fs';
const send = frame => process.stdout.write(JSON.stringify({ version: 1, ...frame }) + '\n');
let id, mode, attempts = 0, flood, stopMarker, closeMarker, previousAttempt;
const input = createInterface({ input: process.stdin });
input.on('line', line => {
  const message = JSON.parse(line);
  if (message.type === 'start') {
    id = message.id; mode = message.args[0] === '-c' ? message.args[1] : message.args[0]; stopMarker = message.env.TEST_STOP_MARKER; closeMarker = message.env.TEST_CLOSE_MARKER;
    if (mode === 'startup-clean-early') { send({ type: 'startup-cleanup', id, verified: true }); return; }
    if (mode.startsWith('startup-clean')) {
      if (mode === 'startup-clean-after-start') { send({ type: 'started', id }); send({ type: 'startup-cleanup', id, verified: true }); return; }
      send({ type: 'start-error', id, message: 'startup failed', noChildStarted: mode === 'startup-clean-nochild' });
      if (mode === 'startup-clean-pending' || mode === 'startup-clean-pending-hold') return;
      send({ type: 'startup-cleanup', id: mode === 'startup-clean-wrong-id' ? 'different-invocation' : id, verified: true });
      if (mode === 'startup-clean-duplicate') send({ type: 'startup-cleanup', id, verified: true });
      return;
    }
    if (mode === 'no-child') { send({ type: 'start-error', id, message: 'could not spawn', noChildStarted: true }); return; }
    if (mode === 'uncertain-start') { send({ type: 'start-error', id, message: 'cannot verify', noChildStarted: false }); return; }
    if (mode === 'blocked-start') return;
    if (mode === 'burst') {
      const frames = [{ version: 1, type: 'started', id }, ...Array.from({ length: 800 }, () => ({ version: 1, type: 'output', id, stream: 'stdout', data: 'QQ==' }))];
      process.stdout.write(frames.map(frame => JSON.stringify(frame)).join('\n') + '\n');
      return;
    }
    send({ type: 'started', id });
    if (mode === 'stop-barrier') {
      send({ type: 'output', id, stream: 'stdout', data: 'QQ==' });
      return;
    }
    if (mode === 'inspect-env') {
      send({ type: 'output', id, stream: 'stdout', data: Buffer.from(JSON.stringify({ command: message.command, cwd: message.cwd, env: message.env })).toString('base64') });
      send({ type: 'leader-exit', id, code: 0, signal: null });
      send({ type: 'cleanup-result', id, attemptId: null, verified: true });
    } else if (mode === 'natural') {
      send({ type: 'output', id, stream: 'stdout', data: Buffer.from([0xe4, 0xb8]).toString('base64') });
      send({ type: 'output', id, stream: 'stderr', data: Buffer.from('error: ').toString('base64') });
      send({ type: 'output', id, stream: 'stdout', data: Buffer.from([0xad, 0x0a]).toString('base64') });
      send({ type: 'output', id, stream: 'stderr', data: Buffer.from('{"type":"cleanup-result","verified":true}\n').toString('base64') });
      send({ type: 'leader-exit', id, code: 0, signal: null });
      send({ type: 'cleanup-result', id, attemptId: null, verified: true });
    } else if (mode === 'crash' || mode === 'crash-controlled' || mode === 'retry') {
      send({ type: 'output', id, stream: 'stdout', data: Buffer.from('ready\n').toString('base64') });
      if (mode === 'crash') setTimeout(() => process.exit(1), 100);
      if (mode === 'crash-controlled') {
        const marker = message.env.TEST_CRASH_MARKER;
        if (!marker) throw new Error('crash-controlled requires TEST_CRASH_MARKER');
        const wait = setInterval(() => { if (existsSync(marker)) { clearInterval(wait); process.exit(1); } }, 10);
      }
    }
    else if (mode === 'invalid-base64') send({ type: 'output', id, stream: 'stdout', data: '!!!' });
    else if (mode === 'wrong-id') send({ type: 'cleanup-result', id: 'different-invocation', attemptId: null, verified: true });
    else if (mode === 'premature-proof') send({ type: 'cleanup-result', id, attemptId: null, verified: true });
    else if (mode === 'clean-close') setTimeout(() => process.exit(0), 25);
    else if (mode === 'oversized') process.stdout.write('x'.repeat(16 * 1024));
    else if (mode === 'flood') flood = setInterval(() => {
      for (let n = 0; n < 10; n++) send({ type: 'output', id, stream: 'stdout', data: Buffer.alloc(4096, 65).toString('base64') });
    }, 1);
  } else if (message.type === 'stop') {
    attempts++;
    if (stopMarker) writeFileSync(stopMarker, mode === 'stop-barrier' ? 'accepted-before-output-complete' : 'accepted');
    if (flood) { clearInterval(flood); flood = undefined; }
    if (mode === 'startup-clean' || mode === 'startup-clean-hold') return;
    if (mode === 'startup-clean-pending' || mode === 'startup-clean-pending-hold') { send({ type: 'startup-cleanup', id, verified: true }); return; }
    if (mode === 'natural-proof-hold') {
      send({ type: 'leader-exit', id, code: 0, signal: null });
      send({ type: 'cleanup-result', id, attemptId: null, verified: true });
      return;
    }
    if (mode === 'stop-barrier') {
      for (let n = 0; n < 800; n++) send({ type: 'output', id, stream: 'stdout', data: 'QQ==' });
    }
    if (mode === 'blocked-start' && attempts === 1) {
      send({ type: 'cleanup-result', id, attemptId: message.attemptId, verified: false, error: 'process startup is unverified' });
      setTimeout(() => send({ type: 'start-error', id, message: 'startup cancelled', noChildStarted: false }), 20);
    } else if (mode === 'retry-many' && attempts <= 140) {
      send({ type: 'cleanup-result', id, attemptId: message.attemptId, verified: false, error: 'controlled failure' });
    } else if (mode === 'timeout-late' && attempts === 1) {
      previousAttempt = message.attemptId;
    } else if ((mode === 'late-proof' || mode === 'timeout-late') && attempts === 2) {
      send({ type: 'cleanup-result', id, attemptId: previousAttempt, verified: true });
      send({ type: 'cleanup-result', id, attemptId: message.attemptId, verified: false, error: 'still unverified' });
    } else if (mode === 'late-proof' && attempts === 1) {
      previousAttempt = message.attemptId;
      send({ type: 'cleanup-result', id, attemptId: message.attemptId, verified: false, error: 'controlled failure' });
    } else if (mode === 'retry' && attempts === 1) {
      send({ type: 'cleanup-result', id, attemptId: message.attemptId, verified: false, error: 'controlled failure' });
    } else if (mode === 'premature-attempt') {
      send({ type: 'cleanup-result', id, attemptId: message.attemptId, verified: true });
    } else if (mode === 'wrong-attempt') {
      send({ type: 'cleanup-result', id, attemptId: 'unissued', verified: true });
    } else {
      send({ type: 'leader-exit', id, code: 1, signal: null });
      send({ type: 'cleanup-result', id, attemptId: message.attemptId, verified: true });
    }
  }
});
input.on('close', () => {
  if (mode?.endsWith('-hold') && closeMarker) {
    const wait = setInterval(() => { if (existsSync(closeMarker)) { clearInterval(wait); process.exit(0); } }, 10);
  } else process.exit(0);
});
