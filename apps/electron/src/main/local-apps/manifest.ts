import { isAbsolute, posix } from 'node:path';
import type { DesktopAppManifest, LocalAppTarget, LocalAppCapability, LocalAppProcess, LocalAppStep } from '../../shared/localApps.js';

export function relativePath(value: unknown, label = 'path'): string {
  if (typeof value !== 'string' || !value || value.includes('\0') || value.includes('\\') || isAbsolute(value)
    || value.split('/').includes('..') || /^[a-z]:/i.test(value)) throw new Error(`Invalid application ${label}`);
  return posix.normalize(value);
}
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Expected an application manifest object');
  return value as Record<string, unknown>;
}
function text(value: unknown, label: string, max = 16384): string {
  if (typeof value !== 'string' || !value.trim() || value.includes('\0') || value.length > max) throw new Error(`Invalid ${label}`);
  return value;
}
function step(value: unknown): LocalAppStep {
  const v = object(value);
  const env: Record<string, string> = {};
  if (v.env !== undefined) for (const [key, value] of Object.entries(object(v.env))) {
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) throw new Error('Invalid environment variable name');
    if (typeof value !== 'string' || value.includes('\0') || value.length > 16384) throw new Error(`Invalid environment variable ${key}`);
    Object.defineProperty(env, key, { value, enumerable: true, configurable: true, writable: true });
  }
  return { command: text(v.command, 'command'), ...(v.cwd !== undefined ? { cwd: relativePath(v.cwd, 'working directory') } : {}),
    ...(v.venv !== undefined ? { venv: relativePath(v.venv, 'virtual environment') } : {}),
    ...(v.python !== undefined ? { python: text(v.python, 'Python version', 32) } : {}),
    ...(v.env !== undefined ? { env } : {}) };
}
function steps(value: unknown): LocalAppStep[] {
  if (value === undefined) return [];
  if (!Array.isArray(value)) throw new Error('Application steps must be an array');
  return value.map(step);
}
export function parseManifest(value: unknown): DesktopAppManifest {
  const v = object(value);
  if (v.schema !== 2) throw new Error('Application manifest schema must be 2; explicitly import legacy apps first');
  const e = object(v.entry);
  const entry: DesktopAppManifest['entry'] = e.type === 'static'
    ? { type: 'static', path: relativePath(e.path, 'entry') }
    : e.type === 'server'
      ? { type: 'server', process: text(e.process, 'entry process', 80), ...(e.path ? { path: text(e.path, 'server entry path', 2048) } : {}) }
      : (() => { throw new Error('Application entry must be static or server'); })();
  if (entry.type === 'server' && entry.path && (!entry.path.startsWith('/') || entry.path.startsWith('//') || /[\\\r\n\0]/.test(entry.path))) throw new Error('Server entry must be a local URL path');
  const processes: LocalAppProcess[] = [];
  if (v.processes !== undefined) {
    if (!Array.isArray(v.processes)) throw new Error('Application processes must be an array');
    for (const raw of v.processes) {
      const p = object(raw);
      const name = text(p.name, 'process name', 80);
      const environmentName = (name: string) => name.toUpperCase().replaceAll('-', '_');
      if (!/^[a-zA-Z][\w-]*$/.test(name) || processes.some(p => environmentName(p.name) === environmentName(name))) throw new Error('Process names must have unique environment identifiers (case and hyphen/underscore insensitive)');
      const process: LocalAppProcess = { ...step(p), name };
      if (p.dependsOn !== undefined) {
        if (!Array.isArray(p.dependsOn)) throw new Error('dependsOn must be an array');
        process.dependsOn = p.dependsOn.map(n => text(n, 'dependency', 80));
      }
      if (p.ready !== undefined) {
        const r = object(p.ready);
        process.ready = {};
        if (r.path !== undefined) {
          const path = text(r.path, 'health path', 2048);
          if (!path.startsWith('/') || path.startsWith('//') || path.includes('://')) throw new Error('Health path must be local');
          process.ready.path = path;
        }
        if (r.output !== undefined) process.ready.output = text(r.output, 'readiness output', 512);
        if (r.timeoutMs !== undefined) {
          if (!Number.isInteger(r.timeoutMs) || Number(r.timeoutMs) < 100 || Number(r.timeoutMs) > 600000) throw new Error('Invalid readiness timeout');
          process.ready.timeoutMs = Number(r.timeoutMs);
        }
      }
      processes.push(process);
    }
  }
  const sorted: LocalAppProcess[] = [];
  const remaining = [...processes];
  while (remaining.length) {
    const at = remaining.findIndex(p => (p.dependsOn ?? []).every(n => sorted.some(s => s.name === n)));
    if (at < 0) throw new Error('Unknown or cyclic process dependencies');
    sorted.push(remaining.splice(at, 1)[0]!);
  }
  if (entry.type === 'server' && !processes.some(p => p.name === entry.process)) throw new Error('Entry process does not exist');
  const capabilities = new Set<LocalAppCapability>();
  const supported: LocalAppCapability[] = ['process', 'files', 'clipboard', 'notifications', 'camera', 'microphone', 'photos', 'share', 'agent', 'background'];
  if (v.capabilities !== undefined) {
    if (!Array.isArray(v.capabilities)) throw new Error('capabilities must be an array');
    for (const cap of v.capabilities) {
      if (!supported.includes(cap as LocalAppCapability)) throw new Error(`Unsupported desktop capability: ${String(cap)}`);
      capabilities.add(cap as LocalAppCapability);
    }
  }
  const install = steps(v.install), build = steps(v.build);
  let agent: DesktopAppManifest['agent'];
  if (v.agent !== undefined) {
    const a = object(v.agent);
    if (Object.keys(a).some(key => key !== 'tools')) throw new Error('Application agent declarations only accept requested tools');
    if (a.tools !== undefined && (!Array.isArray(a.tools) || a.tools.length > 128)) throw new Error('Application agent tools must be an array of at most 128 catalog IDs');
    const tools = (a.tools as unknown[] | undefined ?? []).map(value => text(value, 'agent tool catalog ID', 512));
    if (tools.some(id => !/^(builtin:|mcp:)[A-Za-z0-9_.:/-]+$/.test(id))) throw new Error('Invalid agent tool catalog ID');
    if (new Set(tools).size !== tools.length) throw new Error('Duplicate agent tool catalog ID');
    agent = { tools: tools.sort() };
    capabilities.add('agent');
  }
  if (capabilities.has('agent') && !agent) agent = { tools: [] };
  if (install.length || build.length || processes.length) capabilities.add('process');
  if (!Array.isArray(v.targets) || !v.targets.length) throw new Error('Application targets must be a nonempty array');
  const targets = v.targets.map(raw => {
    const t = object(raw);
    if (!['android', 'ios', 'macos', 'windows', 'linux', 'web'].includes(String(t.platform)) || !['web', 'native', 'mobile-linux'].includes(String(t.environment))) throw new Error('Invalid application target');
    if (t.capabilities !== undefined && (!Array.isArray(t.capabilities) || t.capabilities.some(c => !supported.includes(c as LocalAppCapability)))) throw new Error('Unsupported target capability');
    return t as unknown as LocalAppTarget;
  });
  return { schema: 2, targets, ...(agent ? { agent: { tools: agent.tools } } : {}),
    ...(v.dataSchema ? { dataSchema: v.dataSchema as DesktopAppManifest['dataSchema'] } : {}),
    ...(v.remix ? { remix: v.remix as DesktopAppManifest['remix'] } : {}),
    ...(v.package ? { package: v.package as DesktopAppManifest['package'] } : {}), name: text(v.name, 'application name', 60), summary: typeof v.summary === 'string' ? v.summary.slice(0, 160) : '',
    entry, install, build, processes: sorted, capabilities: [...capabilities].sort() };
}
