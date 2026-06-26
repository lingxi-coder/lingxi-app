#!/usr/bin/env node
/**
 * scripts/e2e.mjs — headless end-to-end proof of the Electron → bridge-server →
 * engine → ClientEvent path, plus the documented entrypoint for a REAL keyed
 * conversation (see clients/README-bridge.md).
 *
 * What it does:
 *   1. Ensures the `bridge-server` binary is built (cargo build if missing,
 *      unless BRIDGE_SERVER_BIN points at an existing binary).
 *   2. Ensures this SDK is built to `dist/` (tsc if missing).
 *   3. Launches the binary as a child process, INHERITING the environment — so
 *      if ANTHROPIC_API_KEY is present in your shell the engine will use it and
 *      you get a real assistant reply. With NO key the engine turn 401s, which
 *      the bridge-server surfaces as a terminal `error` ClientEvent.
 *   4. Connects through the F2-04 discovery lockfile with {@link BridgeClient},
 *      performs the version handshake, sends a prompt, and prints a readable
 *      transcript of every streamed event (text_delta / tool_use_* / turn_ended
 *      / error / …).
 *   5. Shuts the server down cleanly (SIGINT → the binary reaps its lockfile).
 *
 * Two outcomes, both a SUCCESS for this transport proof:
 *   - KEYED   : a `turn_ended` arrives (a real reply streamed first). Prints
 *               'TURN OK (keyed)' and exits 0.
 *   - KEYLESS : no key ⇒ the engine turn errors and a terminal `error`
 *               ClientEvent arrives. This still proves transport + handshake +
 *               the full event path end to end. Prints 'TRANSPORT OK (keyless)'
 *               and exits 0.
 *
 * The script NEVER reads, prints, or logs the API key. It only reports whether
 * one was *present* in the environment (a boolean), never its value.
 *
 * Verify (keyless): `node ./scripts/e2e.mjs` → reaches 'TRANSPORT OK (keyless)'
 * and exits 0.
 */

import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { BridgeClient } from '../dist/index.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

// ── Layout ─────────────────────────────────────────────────────────────────
// scripts/ lives in clients/shared, so:
//   shared       = <repo>/clients/shared
//   repoRoot     = <repo>
//   workspace    = <repo>/lingxi-code   (the cargo workspace)
const sharedDir = resolve(__dirname, '..');
const repoRoot = resolve(sharedDir, '..', '..');
const cargoWorkspace = join(repoRoot, 'lingxi-code');

/** Print a clearly-prefixed status line to stderr (stdout is the transcript). */
function log(msg) {
  process.stderr.write(`[e2e] ${msg}\n`);
}

/** Resolve the bridge-server binary path, building it if necessary. */
function resolveBridgeBinary() {
  const override = process.env.BRIDGE_SERVER_BIN;
  if (override) {
    if (!existsSync(override)) {
      throw new Error(`BRIDGE_SERVER_BIN points at a missing file: ${override}`);
    }
    log(`using BRIDGE_SERVER_BIN=${override}`);
    return override;
  }

  // Prefer release if present, else debug; build debug if neither exists.
  const releaseBin = join(cargoWorkspace, 'target', 'release', 'bridge-server');
  const debugBin = join(cargoWorkspace, 'target', 'debug', 'bridge-server');
  if (existsSync(releaseBin)) {
    log(`found prebuilt release binary: ${releaseBin}`);
    return releaseBin;
  }
  if (existsSync(debugBin)) {
    log(`found prebuilt debug binary: ${debugBin}`);
    return debugBin;
  }

  log('bridge-server binary not found — building (cargo build -p bridge-server)…');
  const build = spawnSync(
    'cargo',
    ['build', '-p', 'bridge-server', '--bin', 'bridge-server'],
    { cwd: cargoWorkspace, stdio: 'inherit' },
  );
  if (build.status !== 0) {
    throw new Error(`cargo build failed (exit ${build.status ?? 'signal'})`);
  }
  if (!existsSync(debugBin)) {
    throw new Error(`build reported success but ${debugBin} is missing`);
  }
  return debugBin;
}

/** Ensure the SDK is compiled to dist/ (this script imports ../dist/index.js). */
function ensureSdkBuilt() {
  const entry = join(sharedDir, 'dist', 'index.js');
  if (existsSync(entry)) {
    return;
  }
  log('SDK dist/ not found — building (npm run build)…');
  const build = spawnSync('npm', ['run', 'build'], { cwd: sharedDir, stdio: 'inherit' });
  if (build.status !== 0) {
    throw new Error(`SDK build failed (exit ${build.status ?? 'signal'})`);
  }
}

/**
 * Launch the bridge-server child (env inherited) and resolve the loopback port
 * once it logs `bridge-server: listening`. The binary roots the engine at the
 * `--cwd` we pass (a throwaway temp dir) so it never indexes this worktree.
 */
function launchServer(binary, cwd) {
  return new Promise((resolvePort, rejectLaunch) => {
    // Inherit env so a present ANTHROPIC_API_KEY is used; force `info` logging so
    // the listening line (with the port) is emitted on stderr, and disable ANSI
    // coloring so the port is parseable from the log line.
    const env = { ...process.env };
    if (!env.RUST_LOG) {
      env.RUST_LOG = 'info';
    }
    env.NO_COLOR = '1';
    env.RUST_LOG_STYLE = 'never';

    const child = spawn(binary, ['--cwd', cwd], {
      cwd,
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    let settled = false;
    const startupTimer = setTimeout(() => {
      if (!settled) {
        settled = true;
        child.kill('SIGKILL');
        rejectLaunch(new Error('bridge-server did not announce a listening port within 30s'));
      }
    }, 30_000);

    // Strip ANSI escape sequences (defensive — some tracing builds color even
    // with NO_COLOR set) so the port is parseable from the log line.
    // eslint-disable-next-line no-control-regex
    const ansi = /\x1b\[[0-9;]*m/g;
    let buffered = '';
    /** Scan accumulated server logs for the bound port. */
    const scan = (chunk) => {
      if (settled) {
        return;
      }
      buffered += chunk.toString().replace(ansi, '');
      // The "listening" line renders `port=<N>` and `lockfile=…/<port>.lock`.
      // Match either signal; the lockfile path is the most unambiguous.
      const m =
        buffered.match(/bridge-server: listening[\s\S]*?\bport[=:]\s*(\d{2,5})/) ??
        buffered.match(/\bport[=:]\s*(\d{2,5})\b[\s\S]*?listening/) ??
        buffered.match(/\/bridge\/(\d{2,5})\.lock\b/);
      if (m) {
        settled = true;
        clearTimeout(startupTimer);
        resolvePort({ child, port: Number.parseInt(m[1], 10) });
      }
    };

    child.stdout.on('data', scan);
    child.stderr.on('data', scan);

    child.once('error', (err) => {
      if (!settled) {
        settled = true;
        clearTimeout(startupTimer);
        rejectLaunch(new Error(`failed to spawn bridge-server: ${err.message}`));
      }
    });
    child.once('exit', (code, signal) => {
      if (!settled) {
        settled = true;
        clearTimeout(startupTimer);
        rejectLaunch(
          new Error(`bridge-server exited before listening (code=${code}, signal=${signal})`),
        );
      }
    });
  });
}

/** Resolve after `ms` milliseconds. */
function delay(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

/** Render one ClientEvent as a single readable transcript line. */
function describeEvent(ev) {
  switch (ev.type) {
    case 'text_delta':
      return `text_delta    ${JSON.stringify(ev.text)}`;
    case 'thinking_delta':
      return `thinking      ${JSON.stringify(ev.thinking)}`;
    case 'tool_use_started':
      return `tool_use      ${ev.tool} id=${ev.id} input=${ev.input_json}`;
    case 'tool_use_result':
      return `tool_result   ${ev.tool} id=${ev.id} is_error=${ev.is_error}`;
    case 'turn_started':
      return `turn_started  turn_id=${ev.turn_id ?? '-'}`;
    case 'message_complete':
      return `msg_complete  stop_reason=${ev.stop_reason ?? '-'}`;
    case 'cost_update':
      return `cost_update   ${ev.formatted} (in=${ev.input_tokens} out=${ev.output_tokens})`;
    case 'turn_ended':
      return `turn_ended    outcome=${ev.outcome.type} stop_reason=${ev.stop_reason ?? '-'} cost=${ev.cost.formatted}`;
    case 'error':
      return `error         kind=${ev.kind.type} message=${JSON.stringify(ev.message)}`;
    case 'session_started':
      return `session       started ${ev.session_id}`;
    default:
      return `${ev.type.padEnd(13)} ${JSON.stringify(ev).slice(0, 200)}`;
  }
}

const PROMPT = 'Reply with exactly: hello from lingxi';

async function main() {
  const hasKey = Boolean(process.env.ANTHROPIC_API_KEY && process.env.ANTHROPIC_API_KEY.length > 0);
  log(`ANTHROPIC_API_KEY present in env: ${hasKey} (value never read/printed)`);

  ensureSdkBuilt();
  const binary = resolveBridgeBinary();

  // A throwaway cwd so the engine roots itself somewhere harmless and the binary
  // does not scan this large worktree.
  const serverCwd = mkdtempSync(join(tmpdir(), 'lingxi-bridge-e2e-'));
  log(`launching bridge-server (cwd=${serverCwd})…`);
  const { child, port } = await launchServer(binary, serverCwd);
  log(`bridge-server listening on 127.0.0.1:${port}`);

  let exitCode = 1;
  let client;
  /** Stop the child cleanly and wait for it to exit (reaps its lockfile). */
  const stopServer = async () => {
    if (child.exitCode !== null || child.signalCode !== null) {
      return;
    }
    await new Promise((r) => {
      const done = () => r();
      child.once('exit', done);
      child.kill('SIGINT');
      // Hard-stop fallback so the script can never hang on shutdown.
      setTimeout(() => {
        child.kill('SIGKILL');
        r();
      }, 5_000);
    });
  };

  try {
    // The binary writes ~/.lingxi/bridge/<port>.lock just after announcing the
    // port. Connect by the EXACT discovered port via an explicit lockfile path so
    // we never race a stale lockfile from another server instance.
    const lockfilePath = join(
      process.env.HOME ?? '',
      '.lingxi',
      'bridge',
      `${port}.lock`,
    );
    // Small grace window for the lockfile write (post-listen, see boot.rs step 4).
    for (let i = 0; i < 50 && !existsSync(lockfilePath); i++) {
      await delay(100);
    }
    if (!existsSync(lockfilePath)) {
      throw new Error(`lockfile never appeared at ${lockfilePath}`);
    }

    client = new BridgeClient({ lockfilePath, clientName: 'lingxi-e2e/0.1.0' });
    client.on('error', (err) => log(`client error: ${err.message}`));

    log('connecting + handshaking…');
    const hello = await client.connect();
    log(
      `handshake OK — server=${hello.server_name} bridge=${hello.protocol_version} client-proto=${hello.capabilities.client_protocol_version}`,
    );

    // Drive one turn and collect the streamed events until a terminal event
    // (turn_ended or error) arrives, or we hit a watchdog timeout.
    log(`sending prompt: ${JSON.stringify(PROMPT)}`);
    process.stdout.write('\n──────── transcript ────────\n');
    client.sendPrompt(PROMPT);

    let sawError = false;
    let sawTurnEnded = false;
    let sawAnyText = false;

    const TURN_WATCHDOG_MS = hasKey ? 120_000 : 30_000;
    const watchdog = new Promise((_, rej) =>
      setTimeout(() => rej(new Error('no terminal event within watchdog window')), TURN_WATCHDOG_MS),
    );

    const drain = (async () => {
      for await (const ev of client.events()) {
        process.stdout.write(`${describeEvent(ev)}\n`);
        if (ev.type === 'text_delta') {
          sawAnyText = true;
        }
        if (ev.type === 'error') {
          sawError = true;
          break;
        }
        if (ev.type === 'turn_ended') {
          sawTurnEnded = true;
          break;
        }
      }
    })();

    await Promise.race([drain, watchdog]);
    process.stdout.write('──────── end transcript ────────\n\n');

    // ── Assertions ────────────────────────────────────────────────────────────
    // Transport + handshake already proven (we got `hello` and streamed frames).
    if (!sawError && !sawTurnEnded) {
      throw new Error('turn produced neither a terminal error nor turn_ended');
    }

    if (hasKey) {
      // Keyed run: a real turn must terminate cleanly. Errors here would be a
      // genuine engine/transport failure worth surfacing.
      if (sawError && !sawTurnEnded) {
        throw new Error(
          'keyed run surfaced an error ClientEvent instead of completing the turn — see transcript above',
        );
      }
      log(`keyed turn completed (streamed text: ${sawAnyText})`);
      process.stdout.write('TURN OK (keyed)\n');
    } else {
      // Keyless run: with no credential the engine turn 401s and the
      // bridge-server surfaces a terminal `error` ClientEvent. That is the PROOF
      // that the whole Electron→bridge→engine→event path is wired end to end.
      if (!sawError) {
        throw new Error(
          'keyless run did not surface the expected terminal error ClientEvent (engine should 401 without a key)',
        );
      }
      log('keyless: handshake + streamed terminal error ClientEvent observed — full path proven');
      process.stdout.write('TRANSPORT OK (keyless)\n');
    }

    exitCode = 0;
  } finally {
    try {
      client?.close();
    } catch {
      /* best-effort */
    }
    await stopServer();
  }

  process.exit(exitCode);
}

main().catch((err) => {
  log(`FAILED: ${err?.stack ?? err}`);
  process.exit(1);
});
