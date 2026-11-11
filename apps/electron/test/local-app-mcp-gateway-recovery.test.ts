import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import type { Tool } from '@modelcontextprotocol/sdk/types.js';
import type { LocalAppMcpManager } from '../src/main/local-apps/mcp.ts';
import { LocalAppMcpGateway, LOCAL_APP_MCP_DESCRIPTOR } from '../src/main/local-apps/mcpGateway.ts';

test('gateway refreshes tools on a real SSE reattachment after offline catalog changes', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'lingxi-mcp-gateway-recovery-'));
  let tools: Tool[] = [];
  let changed: (() => void) | undefined;
  const manager = {
    tools: () => tools,
    onChanged: (listener: () => void) => { changed = listener; return () => { changed = undefined; }; },
    call: async () => ({ content: [] }),
  } as unknown as LocalAppMcpManager;
  const gateway = new LocalAppMcpGateway(root, manager);
  const aborts: AbortController[] = [];
  t.after(async () => {
    for (const abort of aborts) abort.abort();
    await gateway.dispose();
    await fs.rm(root, { recursive: true, force: true });
  });
  await gateway.initialize();
  const descriptor = JSON.parse(await fs.readFile(join(root, LOCAL_APP_MCP_DESCRIPTOR), 'utf8'));
  const url = `http://127.0.0.1:${descriptor.port}/mcp`;
  const headers: Record<string, string> = {
    Authorization: `Bearer ${descriptor.token}`,
    Accept: 'application/json, text/event-stream',
    'Content-Type': 'application/json',
  };
  const init = await fetch(url, {
    method: 'POST', headers,
    body: JSON.stringify({ jsonrpc: '2.0', id: 0, method: 'initialize', params: {
      protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'recovery-fixture', version: '1' },
    } }),
    signal: AbortSignal.timeout(5000),
  });
  assert.equal(init.status, 200);
  const initialized = await init.json() as { result: { protocolVersion: string } };
  assert.ok(init.headers.get('mcp-session-id'));
  headers['mcp-session-id'] = init.headers.get('mcp-session-id')!;
  headers['mcp-protocol-version'] = initialized.result.protocolVersion;
  async function catalog(id: number) {
    const response = await fetch(url, {
      method: 'POST', headers, body: JSON.stringify({ jsonrpc: '2.0', id, method: 'tools/list', params: {} }),
      signal: AbortSignal.timeout(5000),
    });
    assert.equal(response.status, 200);
    return (await response.json() as { result: { tools: Tool[] } }).result.tools;
  }
  async function attach() {
    const abort = new AbortController(); aborts.push(abort);
    for (let attempt = 0; ; attempt++) {
      const response = await fetch(url, { headers, signal: AbortSignal.any([abort.signal, AbortSignal.timeout(5000)]) });
      if (response.status === 409 && attempt < 100) { await response.text(); await delay(10); continue; }
      assert.equal(response.status, 200);
      assert.match(response.headers.get('content-type') ?? '', /^text\/event-stream/);
      const reader = response.body!.getReader();
      let text = '';
      while (!text.includes('\n\n')) {
        const part = await reader.read();
        assert.equal(part.done, false);
        text += new TextDecoder().decode(part.value);
      }
      const data = text.split('\n').find(line => line.startsWith('data:'));
      assert.ok(data, text);
      // This is an actual server notification; the client does not synthesize it.
      assert.deepEqual(JSON.parse(data.slice(5)), { method: 'notifications/tools/list_changed', jsonrpc: '2.0' });
      return { abort, reader };
    }
  }
  const first = await attach();
  assert.deepEqual(await catalog(1), []);
  first.abort.abort();
  await first.reader.cancel().catch(() => undefined);
  tools = [{ name: 'published_while_disconnected', inputSchema: { type: 'object' } }];
  changed!();
  const second = await attach();
  assert.deepEqual(await catalog(2), tools);
  second.abort.abort();
  await second.reader.cancel().catch(() => undefined);
  // An unsuccessful GET must not be treated as a stream attachment.
  const rejected = await fetch(url, { headers: { ...headers, Accept: 'application/json' }, signal: AbortSignal.timeout(5000) });
  assert.equal(rejected.status, 406);
  await rejected.text();
  tools = [];
  changed!();
  await attach();
  assert.deepEqual(await catalog(3), []);
});
