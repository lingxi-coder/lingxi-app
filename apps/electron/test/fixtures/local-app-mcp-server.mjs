import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import { ListToolsRequestSchema, CallToolRequestSchema, McpError, ErrorCode, isInitializeRequest } from '@modelcontextprotocol/sdk/types.js';
import { createServer } from 'node:http';
import { randomUUID } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

if (process.argv[2] === 'unresponsive') { process.stdin.resume(); setInterval(() => {}, 1000); }
else {
  const counterPath = join(process.env.LOCAL_APP_DATA_DIR, 'mcp-count');
  await writeFile(join(process.env.LOCAL_APP_DATA_DIR, 'mcp-pid'), String(process.pid));
  const readCount = async () => Number(await readFile(counterPath, 'utf8').catch(() => '0'));
  const result = value => ({ content: [{ type: 'text', text: String(value) }] });
  function makeServer() {
    let extra = false;
    const server = new Server({ name: 'app-fixture', version: '1.0.0' }, { capabilities: { tools: { listChanged: true } } });
    server.setRequestHandler(ListToolsRequestSchema, () => ({ tools: ['read', 'increment', 'crash', 'slow', 'change', 'invalid', ...(extra ? ['extra'] : [])].map(name => ({ name, description: name, inputSchema: { type: 'object', properties: {}, additionalProperties: false } })) }));
    server.setRequestHandler(CallToolRequestSchema, async (request, context) => {
      const name = request.params.name;
      if (name === 'invalid') throw new McpError(ErrorCode.InvalidParams, 'Invalid domain argument');
      if (name === 'increment' || name === 'crash') {
        await writeFile(counterPath, String(await readCount() + 1));
        if (name === 'crash') process.exit(17);
      }
      if (name === 'slow') {
        await writeFile(join(process.env.LOCAL_APP_DATA_DIR, 'mcp-slow-started'), 'yes');
        await new Promise((resolve, reject) => {
          const timer = setTimeout(resolve, 30_000);
          context.signal.addEventListener('abort', () => { clearTimeout(timer); reject(new Error('cancelled')); }, { once: true });
        });
      }
      if (name === 'change') { extra = true; await server.sendToolListChanged(); }
      return result(await readCount());
    });
    return server;
  }
  if (process.argv[2] === 'http') {
    const sessions = new Map();
    createServer(async (request, response) => {
      if (request.url === '/drop-events') { for (const transport of sessions.values()) transport.closeStandaloneSSEStream(); response.end('dropped'); return; }
      if (request.url === '/health') { response.end('ready'); return; }
      if (request.url !== '/mcp' || request.headers.origin || request.headers.authorization !== `Bearer ${process.env.LOCAL_APP_MCP_TOKEN}`) { response.writeHead(403).end(); return; }
      try {
        let body;
        if (request.method === 'POST') { const parts = []; for await (const part of request) parts.push(part); body = JSON.parse(Buffer.concat(parts).toString()); }
        const id = request.headers['mcp-session-id'];
        let transport = sessions.get(id);
        if (!transport && isInitializeRequest(body)) {
          const server = makeServer();
          transport = new StreamableHTTPServerTransport({ sessionIdGenerator: randomUUID, enableJsonResponse: true,
            onsessioninitialized: id => sessions.set(id, transport), onsessionclosed: id => sessions.delete(id) });
          await server.connect(transport);
        }
        if (!transport) { response.writeHead(404).end(); return; }
        await transport.handleRequest(request, response, body);
      } catch { if (!response.headersSent) response.writeHead(500); response.end(); }
    }).listen(Number(process.env.PORT), '127.0.0.1');
  } else await makeServer().connect(new StdioServerTransport());
}
