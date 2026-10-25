import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { WebSocketServer } from 'ws';
const server = createServer(async (req, res) => {
  const path = new URL(req.url, 'http://fixture').pathname;
  if (path === '/health') { res.end('NODE-READY'); return; }
  if (path === '/redirect') { res.writeHead(302, { Location: '/health' }); res.end(); return; }
  if (path === '/echo' && req.method === 'POST') { res.setHeader('Content-Type', 'application/octet-stream'); req.pipe(res); return; }
  if (path === '/events') {
    res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' });
    let n = 0; res.write(`data: ${n++}\n\n`);
    const timer = setInterval(() => { res.write(`data: ${n++}\n\n`); if (n === 3) { clearInterval(timer); res.end(); } }, 150);
    res.on('close', () => clearInterval(timer)); return;
  }
  const file = path === '/' ? 'index.html' : path === '/client.js' ? 'client.js' : null;
  if (!file) { res.writeHead(404); res.end(); return; }
  res.setHeader('Content-Type', file.endsWith('.js') ? 'text/javascript' : 'text/html');
  try { res.end(await readFile(new URL(file, import.meta.url))); }
  catch { res.writeHead(500); res.end(); }
});
const sockets = new WebSocketServer({ server, path: '/socket', maxPayload: 1048576 });
sockets.on('connection', socket => socket.on('message', (data, binary) => socket.send(data, { binary })));
server.listen(Number(process.env.PORT), process.env.HOST);
