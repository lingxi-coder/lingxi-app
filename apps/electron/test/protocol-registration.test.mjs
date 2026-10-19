import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { writeProtocolRegistration, verifyProtocolRegistration, WINDOWS_PROTOCOL_SCRIPT } from '../scripts/protocol-registration.mjs';

test('Linux registration quotes executable paths, uses only XDG user state, restores previous owner and unregisters idempotently', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-protocol-'));
  try {
    const payload = join(root, 'App "quoted" %name $money `literal`');
    const bin = join(root, 'bin');
    const data = join(root, 'data');
    const config = join(root, 'config');
    for (const path of [payload, bin, config]) mkdirSync(path, { recursive: true });
    writeFileSync(join(payload, 'lingxi-code'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
    writeFileSync(join(bin, 'xdg-mime'), '#!/bin/sh\nif [ "$1" = query ]; then if [ -f "$STATE" ]; then cat "$STATE"; else echo previous.desktop; fi; else echo "$2" > "$STATE"; fi\n', { mode: 0o755 });
    const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, XDG_DATA_HOME: data, XDG_CONFIG_HOME: config, STATE: join(root, 'state') };
    writeProtocolRegistration(payload, 'linux');
    verifyProtocolRegistration(payload, 'linux');
    const run = action => execFileSync('/bin/sh', [join(payload, 'register-protocol.sh'), action], { env, encoding: 'utf8' });
    run('register');
    const desktop = join(data, 'applications', 'lingxi-code.desktop');
    const entry = readFileSync(desktop, 'utf8');
    assert.ok(entry.includes('Exec="'));
    assert.ok(entry.includes('\\"quoted\\" %%name'));
    assert.ok(entry.includes('\\$money \\`literal\\`/lingxi-code" %u'));
    assert.ok(entry.includes('MimeType=x-scheme-handler/lingxi;'));
    assert.equal(readFileSync(env.STATE, 'utf8').trim(), 'lingxi-code.desktop');
    run('register');
    writeFileSync(join(config, 'mimeapps.list'), '[Default Applications]\nx-scheme-handler/lingxi=lingxi-code.desktop;previous.desktop;\ntext/plain=editor.desktop;\n');
    run('unregister');
    assert.equal(existsSync(desktop), false);
    assert.equal(readFileSync(env.STATE, 'utf8').trim(), 'previous.desktop');
    assert.match(readFileSync(join(config, 'mimeapps.list'), 'utf8'), /x-scheme-handler\/lingxi=previous\.desktop;/);
    assert.match(readFileSync(join(config, 'mimeapps.list'), 'utf8'), /text\/plain=editor\.desktop;/);
    run('unregister');
    writeFileSync(desktop, '[Desktop Entry]\nX-LingXi-Executable=/another/app\n');
    assert.throws(() => run('register'), /Another installation owns/);
    assert.throws(() => run('unregister'), /Another installation owns/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('portable Windows helper targets HKCU, quotes URI as one argument and refuses to remove another installation', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-protocol-win-'));
  try {
    writeProtocolRegistration(root, 'win32');
    verifyProtocolRegistration(root, 'win32');
    assert.match(WINDOWS_PROTOCOL_SCRIPT, /HKCU:\\Software\\Classes\\lingxi/);
    assert.match(WINDOWS_PROTOCOL_SCRIPT, /'"' \+ \$Executable \+ '" "%1"'/);
    assert.match(WINDOWS_PROTOCOL_SCRIPT, /\$Current -ne \$Command/);
    assert.match(WINDOWS_PROTOCOL_SCRIPT, /ValidateSet\('Register','Unregister'\)/);
    assert.doesNotMatch(WINDOWS_PROTOCOL_SCRIPT, /HKLM:|RunAs|Start-Process|Invoke-Expression/);
    writeFileSync(join(root, 'register-protocol.ps1'), 'stale');
    assert.throws(() => verifyProtocolRegistration(root, 'win32'), /missing or stale/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
