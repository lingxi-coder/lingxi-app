"""Deliver isolated Chromium's actual PNG frames to one synthetic browser on another regional VM."""

import base64
import hashlib
import http.server
import ipaddress
import json
import ssl
import struct
import subprocess
import threading
import time
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import ProxyHandler, Request, build_opener

META = "http://metadata.google.internal/computeMetadata/v1/instance/"
PORT = 18444
direct = build_opener(ProxyHandler({}))


def metadata(path, value=None):
    req = Request(
        META + path,
        data=value,
        method="PUT" if value is not None else "GET",
        headers={"Metadata-Flavor": "Google"},
    )
    with direct.open(req, timeout=4) as res:
        return res.read().decode().strip()


def wait_attribute(name, timeout=360):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        try:
            return metadata("attributes/" + name)
        except HTTPError as exc:
            if exc.code != 404:
                raise
        time.sleep(2)
    raise TimeoutError("Missing peer readiness attribute: " + name)


def serve():
    host = str(ipaddress.IPv4Address(metadata("network-interfaces/0/ip")))
    tls = Path("tls")
    extensions = tls / "wire.ext"
    extensions.write_text(
        "basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:"
        + host
        + "\n"
    )
    subprocess.run(
        [
            "openssl",
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=Dataset wire fixture",
            "-keyout",
            tls / "wire.key",
            "-out",
            tls / "wire.csr",
        ],
        check=True,
    )
    subprocess.run(
        [
            "openssl",
            "x509",
            "-req",
            "-in",
            tls / "wire.csr",
            "-CA",
            tls / "ca.crt",
            "-CAkey",
            tls / "ca.key",
            "-CAcreateserial",
            "-days",
            "1",
            "-extfile",
            extensions,
            "-out",
            tls / "wire.crt",
        ],
        check=True,
    )
    metadata("guest-attributes/render/wire-ca", (tls / "ca.crt").read_bytes())
    peer = str(ipaddress.IPv4Address(wait_attribute("client-ip")))
    if (
        wait_attribute("client-ready") != "true"
        or not ipaddress.ip_address(peer).is_private
    ):
        raise ValueError("Unexpected synthetic browser peer")
    viewer = Path("wire-viewer.html").read_bytes()

    def exchange(child):
        lock, finished = threading.Lock(), threading.Event()
        tally = {"frames": 0, "pngBytes": 0}
        errors = []
        start = time.monotonic()
        origin = f"https://{host}:{PORT}"

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args):
                pass

            def send_body(self, data, content_type, **headers):
                self.send_response(200)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(data)))
                self.send_header("Cache-Control", "no-store")
                self.send_header("X-Content-Type-Options", "nosniff")
                for name, value in headers.items():
                    self.send_header(name, str(value))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                if self.path != "/viewer" or self.client_address[0] != peer:
                    self.send_error(404)
                    return
                self.send_body(viewer, "text/html; charset=utf-8")

            def do_POST(self):
                if (
                    self.client_address[0] != peer
                    or self.headers.get("Origin") != origin
                    or self.path not in ("/frame", "/finish")
                ):
                    self.send_error(403)
                    return
                try:
                    length = int(self.headers["Content-Length"])
                    if not 0 < length <= 64:
                        self.send_error(400)
                        return
                    body = json.loads(self.rfile.read(length))
                    with lock:
                        if self.path == "/finish":
                            if body != {"frames": 120} or tally["frames"] != 120:
                                raise ValueError("Incomplete viewer session")
                            self.send_body(b"{}", "application/json")
                            finished.set()
                            return
                        n = tally["frames"] + 1
                        if body != {"n": n} or n > 120:
                            self.send_error(409)
                            return
                        child.stdin.write(json.dumps({"type": "tap", "n": n}) + "\n")
                        child.stdin.flush()
                        result = json.loads(child.stdout.readline())
                        frame = base64.b64decode(result["png"], validate=True)
                        digest = hashlib.sha256(frame).hexdigest()
                        if (
                            result["phase"] != "screen"
                            or result["n"] != n
                            or result["quantity"] != 8 + n
                            or result["sha256"] != digest
                            or frame[:8] != bytes.fromhex("89504e470d0a1a0a")
                            or struct.unpack("!II", frame[16:24]) != (480, 640)
                        ):
                            raise ValueError("Invalid rendered frame")
                        self.send_body(
                            frame,
                            "image/png",
                            **{"X-Frame-SHA256": digest, "X-Quantity": 8 + n},
                        )
                        tally["frames"] = n
                        tally["pngBytes"] += len(frame)
                except Exception as exc:
                    errors.append(type(exc).__name__)
                    finished.set()
                    self.close_connection = True

        with http.server.ThreadingHTTPServer((host, PORT), Handler) as server:
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.load_cert_chain(tls / "wire.crt", tls / "wire.key")
            server.socket = context.wrap_socket(server.socket, server_side=True)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                if not finished.wait(622) or errors:
                    raise RuntimeError("Viewer exchange failed: " + str(errors))
            finally:
                server.shutdown()
                thread.join()
        child.stdin.write('{"type":"close"}\n')
        child.stdin.flush()
        return {
            **tally,
            "seconds": round(time.monotonic() - start, 2),
            "peer": peer,
            "tls": True,
        }

    return exchange
