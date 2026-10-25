"""Local fixture only. Exercise request bytes, streaming, and durable app data."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json, os, time
from pathlib import Path

class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def do_GET(self):
        if self.path == '/events':
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Connection', 'close')
            self.end_headers()
            for n in range(3):
                self.wfile.write(f'data: {n}\n\n'.encode()); self.wfile.flush(); time.sleep(.15)
            self.close_connection = True
            return
        if self.path == '/redirect':
            self.send_response(302); self.send_header('Location', '/health'); self.send_header('Content-Length', '0'); self.end_headers(); return
        if self.path == '/health':
            body = b'PYTHON-READY'
        else:
            body = b'<!doctype html><html><head><title>Python acceptance</title></head><body><h1>Python service ready</h1></body></html>'
        self.send_response(200); self.send_header('Content-Length', str(len(body))); self.send_header('Content-Type', 'text/html' if self.path != '/health' else 'text/plain'); self.end_headers(); self.wfile.write(body)
    def do_POST(self):
        size = int(self.headers.get('Content-Length', '0'))
        if size > 1048576: self.send_error(413); return
        body = self.rfile.read(size)
        if self.path == '/save':
            data = Path(os.environ['LOCAL_APP_DATA_DIR']); data.mkdir(parents=True, exist_ok=True)
            (data / 'acceptance.bin').write_bytes(body)
        self.send_response(200); self.send_header('Content-Type', 'application/octet-stream'); self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)

ThreadingHTTPServer((os.environ['HOST'], int(os.environ['PORT'])), Handler).serve_forever()
