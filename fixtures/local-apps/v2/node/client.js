document.querySelector('#run').addEventListener('click', async () => {
  const status = document.querySelector('#status'); status.textContent = '';
  const log = text => { status.textContent += text + '\n'; };
  try {
    const payload = new Uint8Array([0, 1, 128, 255]);
    const echo = await fetch(new URL('echo', location.href), { method: 'POST', body: payload });
    const bytes = new Uint8Array(await echo.arrayBuffer());
    if (String(bytes) !== String(payload)) throw new Error('POST byte mismatch'); log('POST passed');
    await new Promise((resolve, reject) => {
      const stream = new EventSource(new URL('events', location.href)); const seen = [];
      const timer = setTimeout(() => { stream.close(); reject(new Error('SSE timeout')); }, 5000);
      stream.onmessage = event => { seen.push(event.data); if (seen.length === 3) { clearTimeout(timer); stream.close(); if (seen.join() !== '0,1,2') reject(new Error('SSE mismatch')); else resolve(); } };
    }); log('SSE passed');
    const url = await window.localApp.serviceSocketUrl('api', '/socket');
    await new Promise((resolve, reject) => {
      const socket = new WebSocket(url); socket.binaryType = 'arraybuffer';
      const timer = setTimeout(() => { socket.close(); reject(new Error('WebSocket timeout')); }, 5000);
      socket.onopen = () => socket.send(payload);
      socket.onerror = () => { clearTimeout(timer); reject(new Error('WebSocket failed')); };
      socket.onmessage = event => { clearTimeout(timer); socket.close(); if (String(new Uint8Array(event.data)) === String(payload)) resolve(); else reject(new Error('WebSocket byte mismatch')); };
    }); log('Binary WebSocket passed');
  } catch (error) { log(`FAILED: ${error.message}`); }
});
