"""Opt-in reverse proxy for timing/SSE diagnostics with content-free persistence."""

from __future__ import annotations

import hashlib
import http.client
import base64
import os
import selectors
import socket
import ssl
import subprocess
import tempfile
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlsplit
from urllib.request import getproxies, proxy_bypass

from .models import NetworkFlowRecord, Quality
from .storage import Storage


HOP_BY_HOP = {
    "connection", "keep-alive", "proxy-authenticate", "proxy-authorization",
    "te", "trailers", "transfer-encoding", "upgrade", "host", "content-length",
}
MAX_REQUEST_BYTES = 64 * 1024 * 1024


class ReverseProxyServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, address: tuple[str, int], storage: Storage, upstream: str) -> None:
        parsed = urlsplit(upstream)
        if parsed.scheme not in ("http", "https") or not parsed.hostname:
            raise ValueError("upstream must be an http(s) URL")
        self.storage = storage
        self.upstream_scheme = parsed.scheme
        self.upstream_host = parsed.hostname
        self.upstream_port = parsed.port or (443 if parsed.scheme == "https" else 80)
        self.upstream_prefix = parsed.path.rstrip("/")
        super().__init__(address, ReverseProxyHandler)


class ReverseProxyHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server: ReverseProxyServer

    def do_GET(self) -> None:  # noqa: N802
        self._forward()

    def do_POST(self) -> None:  # noqa: N802
        self._forward()

    def do_DELETE(self) -> None:  # noqa: N802
        self._forward()

    def do_PATCH(self) -> None:  # noqa: N802
        self._forward()

    def do_PUT(self) -> None:  # noqa: N802
        self._forward()

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def _forward(self) -> None:
        if self.headers.get("Upgrade", "").lower() == "websocket":
            self._forward_websocket()
            return
        started_wall = datetime.now(timezone.utc)
        started = time.monotonic()
        connection: http.client.HTTPConnection | None = None
        response_bytes = 0
        first_event_ms = first_output_ms = ttfb_ms = None
        request_size = 0
        status = None
        error_type = None
        success = False
        try:
            length = int(self.headers.get("Content-Length", "0"))
            request_size = length
            if length < 0 or length > MAX_REQUEST_BYTES:
                self.send_error(413)
                return
            request_body = self.rfile.read(length) if length else None
            connection, proxy_headers, absolute_form = _upstream_connection(self.server)
            headers = {
                key: value for key, value in self.headers.items()
                if key.lower() not in HOP_BY_HOP
            }
            headers.update(proxy_headers)
            path = self.server.upstream_prefix + self.path
            if absolute_form:
                path = f"{self.server.upstream_scheme}://{self.server.upstream_host}:{self.server.upstream_port}{path}"
            connection.request(self.command, path, body=request_body, headers=headers)
            upstream = connection.getresponse()
            ttfb_ms = (time.monotonic() - started) * 1000
            status = upstream.status
            if os.environ.get("CODEX_METER_DEBUG_PROXY") == "1":
                safe_path = self.path.split("?", 1)[0][:256]
                print(
                    "proxy",
                    self.command,
                    safe_path,
                    upstream.status,
                    "length=" + str(upstream.getheader("Content-Length")),
                    "transfer=" + str(upstream.getheader("Transfer-Encoding")),
                    "type=" + str(upstream.getheader("Content-Type")),
                    file=os.sys.stderr,
                    flush=True,
                )
            self.send_response(upstream.status, upstream.reason)
            for key, value in upstream.getheaders():
                if key.lower() not in HOP_BY_HOP:
                    self.send_header(key, value)
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()
            scanner = _SseTimingScanner(started)
            while True:
                chunk = upstream.read(16384)
                if not chunk:
                    break
                response_bytes += len(chunk)
                scanner.feed(chunk)
                self.wfile.write(f"{len(chunk):X}\r\n".encode("ascii"))
                self.wfile.write(chunk)
                self.wfile.write(b"\r\n")
                self.wfile.flush()
            self.wfile.write(b"0\r\n\r\n")
            first_event_ms = scanner.first_event_ms
            first_output_ms = scanner.first_output_ms
            success = 200 <= upstream.status < 400
        except (OSError, http.client.HTTPException, ssl.SSLError, ValueError) as error:
            error_type = type(error).__name__
            if not self.wfile.closed:
                try:
                    self.send_error(502)
                except OSError:
                    pass
            ttfb_ms = None
        finally:
            if connection is not None:
                connection.close()
            ended = datetime.now(timezone.utc)
            flow = NetworkFlowRecord(
                event_fingerprint="", mode="tls_reverse_proxy" if isinstance(self.request, ssl.SSLSocket) else "reverse_proxy",
                data_source="local_reverse_proxy", started_at=_iso(started_wall), ended_at=_iso(ended),
                destination_host=self.server.upstream_host, destination_port=self.server.upstream_port,
                protocol=f"{self.server.upstream_scheme}/http1.1", http_status=status,
                request_bytes=max(0, request_size), response_bytes=response_bytes,
                ttfb_ms=ttfb_ms, first_event_ms=first_event_ms, first_output_ms=first_output_ms,
                duration_ms=(time.monotonic() - started) * 1000, success=success,
                error_type=error_type, quality=Quality("local_reverse_proxy"),
            )
            flow.event_fingerprint = _fingerprint(flow)
            self.server.storage.insert_network_flow(flow)
            self.close_connection = True

    def _forward_websocket(self) -> None:
        started_wall = datetime.now(timezone.utc)
        started = time.monotonic()
        upstream: socket.socket | None = None
        request_bytes = response_bytes = 0
        status = None
        success = False
        error_type = None
        try:
            upstream = _open_upstream_socket(self.server)
            path = self.server.upstream_prefix + self.path
            lines = [f"{self.command} {path} HTTP/1.1", f"Host: {self.server.upstream_host}"]
            for key, value in self.headers.items():
                if key.lower() not in ("host", "proxy-authorization"):
                    lines.append(f"{key}: {value}")
            request = ("\r\n".join(lines) + "\r\n\r\n").encode("latin-1")
            upstream.sendall(request)
            header = _read_socket_header(upstream)
            first_line = header.split(b"\r\n", 1)[0].decode("ascii", "replace")
            parts = first_line.split(" ", 2)
            status = int(parts[1]) if len(parts) > 1 and parts[1].isdigit() else None
            self.request.sendall(header)
            success = status == 101
            if success:
                request_bytes, response_bytes = _relay_sockets(self.request, upstream)
        except (OSError, ssl.SSLError, ValueError) as error:
            error_type = type(error).__name__
            try:
                self.request.sendall(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            except OSError:
                pass
        finally:
            if upstream is not None:
                upstream.close()
            ended = datetime.now(timezone.utc)
            flow = NetworkFlowRecord(
                event_fingerprint="", mode="websocket_reverse_proxy",
                data_source="local_reverse_proxy", started_at=_iso(started_wall), ended_at=_iso(ended),
                destination_host=self.server.upstream_host, destination_port=self.server.upstream_port,
                protocol="wss", http_status=status, request_bytes=request_bytes,
                response_bytes=response_bytes, duration_ms=(time.monotonic() - started) * 1000,
                success=success, error_type=error_type, quality=Quality("local_reverse_proxy"),
            )
            flow.event_fingerprint = _fingerprint(flow)
            self.server.storage.insert_network_flow(flow)
            self.close_connection = True


class _SseTimingScanner:
    def __init__(self, started: float) -> None:
        self.started = started
        self.buffer = bytearray()
        self.first_event_ms: float | None = None
        self.first_output_ms: float | None = None

    def feed(self, chunk: bytes) -> None:
        if self.first_output_ms is not None:
            return
        self.buffer.extend(chunk)
        if len(self.buffer) > 65536:
            del self.buffer[:-65536]
        while b"\n" in self.buffer:
            line, _, remaining = self.buffer.partition(b"\n")
            self.buffer = bytearray(remaining)
            if line.startswith(b"event:"):
                elapsed = (time.monotonic() - self.started) * 1000
                if self.first_event_ms is None:
                    self.first_event_ms = elapsed
                event = line[6:].strip()
                if event in (
                    b"response.output_text.delta", b"response.content_part.added",
                    b"response.output_item.added", b"response.completed",
                ):
                    self.first_output_ms = elapsed


def wrap_server_tls(server: ReverseProxyServer, certificate: Path, private_key: Path) -> None:
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(certificate, private_key)
    server.socket = context.wrap_socket(server.socket, server_side=True)


def _upstream_connection(
    server: ReverseProxyServer,
) -> tuple[http.client.HTTPConnection, dict[str, str], bool]:
    proxy_url = None if proxy_bypass(server.upstream_host) else getproxies().get(server.upstream_scheme)
    proxy_headers: dict[str, str] = {}
    if proxy_url:
        parsed = urlsplit(proxy_url)
        if parsed.hostname:
            if parsed.username is not None:
                credentials = f"{unquote(parsed.username)}:{unquote(parsed.password or '')}".encode()
                proxy_headers["Proxy-Authorization"] = "Basic " + base64.b64encode(credentials).decode("ascii")
            proxy_port = parsed.port or (443 if parsed.scheme == "https" else 80)
            if server.upstream_scheme == "https":
                connection = http.client.HTTPSConnection(
                    parsed.hostname, proxy_port, timeout=120, context=ssl.create_default_context(),
                )
                connection.set_tunnel(
                    server.upstream_host, server.upstream_port,
                    headers=proxy_headers or None,
                )
                return connection, {}, False
            return http.client.HTTPConnection(parsed.hostname, proxy_port, timeout=120), proxy_headers, True
    if server.upstream_scheme == "https":
        return (
            http.client.HTTPSConnection(
                server.upstream_host, server.upstream_port,
                timeout=120, context=ssl.create_default_context(),
            ),
            {}, False,
        )
    return http.client.HTTPConnection(server.upstream_host, server.upstream_port, timeout=120), {}, False


def _open_upstream_socket(server: ReverseProxyServer) -> socket.socket:
    proxy_url = None if proxy_bypass(server.upstream_host) else getproxies().get(server.upstream_scheme)
    raw: socket.socket
    if proxy_url:
        parsed = urlsplit(proxy_url)
        if not parsed.hostname:
            raise OSError("invalid upstream proxy")
        raw = socket.create_connection((parsed.hostname, parsed.port or 80), timeout=30)
        headers = [
            f"CONNECT {server.upstream_host}:{server.upstream_port} HTTP/1.1",
            f"Host: {server.upstream_host}:{server.upstream_port}",
        ]
        if parsed.username is not None:
            credentials = f"{unquote(parsed.username)}:{unquote(parsed.password or '')}".encode()
            headers.append("Proxy-Authorization: Basic " + base64.b64encode(credentials).decode("ascii"))
        raw.sendall(("\r\n".join(headers) + "\r\n\r\n").encode("latin-1"))
        response = _read_socket_header(raw)
        first_line = response.split(b"\r\n", 1)[0]
        if b" 200 " not in first_line:
            raw.close()
            raise OSError("upstream CONNECT failed")
    else:
        raw = socket.create_connection((server.upstream_host, server.upstream_port), timeout=30)
    if server.upstream_scheme == "https":
        raw = ssl.create_default_context().wrap_socket(raw, server_hostname=server.upstream_host)
    raw.settimeout(None)
    return raw


def _read_socket_header(sock: socket.socket) -> bytes:
    data = bytearray()
    while b"\r\n\r\n" not in data:
        chunk = sock.recv(4096)
        if not chunk:
            break
        data.extend(chunk)
        if len(data) > 65536:
            raise ValueError("upstream header too large")
    if b"\r\n\r\n" not in data:
        raise OSError("incomplete upstream header")
    return bytes(data)


def _relay_sockets(client: socket.socket, upstream: socket.socket) -> tuple[int, int]:
    selector = selectors.DefaultSelector()
    selector.register(client, selectors.EVENT_READ, (client, upstream, "out"))
    selector.register(upstream, selectors.EVENT_READ, (upstream, client, "in"))
    totals = {"out": 0, "in": 0}
    while selector.get_map():
        events = selector.select(timeout=120)
        if not events:
            break
        for key, _ in events:
            source, target, direction = key.data
            try:
                chunk = source.recv(65536)
            except OSError:
                chunk = b""
            if not chunk:
                selector.unregister(source)
                try:
                    target.shutdown(socket.SHUT_WR)
                except OSError:
                    pass
                continue
            target.sendall(chunk)
            totals[direction] += len(chunk)
    selector.close()
    return totals["out"], totals["in"]


def initialize_tls_material(directory: Path) -> dict[str, Path]:
    """Create a local CA and localhost leaf certificate without overwriting existing keys."""
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    paths = {
        "ca_cert": directory / "codex-meter-ca.pem",
        "ca_key": directory / "codex-meter-ca-key.pem",
        "leaf_cert": directory / "localhost.pem",
        "leaf_key": directory / "localhost-key.pem",
    }
    if all(path.exists() for path in paths.values()):
        _ensure_certificate_chain(paths["leaf_cert"], paths["ca_cert"])
        return paths
    if any(path.exists() for path in paths.values()):
        raise FileExistsError("partial TLS material exists; move it aside before regenerating")
    with tempfile.TemporaryDirectory(prefix="codex-meter-tls-") as temp_name:
        temp = Path(temp_name)
        ca_key = temp / "ca-key.pem"
        ca_cert = temp / "ca.pem"
        leaf_key = temp / "leaf-key.pem"
        leaf_csr = temp / "leaf.csr"
        leaf_cert = temp / "leaf.pem"
        ca_database = temp / "index.txt"
        ca_serial = temp / "serial"
        new_certificates = temp / "newcerts"
        openssl_config = temp / "openssl.cnf"
        ca_database.write_text("", encoding="utf-8")
        ca_serial.write_text("1000\n", encoding="ascii")
        new_certificates.mkdir()
        openssl_config.write_text(
            "[ req ]\n"
            "distinguished_name = ca_dn\n"
            "x509_extensions = v3_ca\n"
            "prompt = no\n"
            "[ ca_dn ]\n"
            "CN = Codex Meter Local Diagnostic CA\n"
            "[ v3_ca ]\n"
            "basicConstraints = critical,CA:TRUE\n"
            "keyUsage = critical,keyCertSign,cRLSign\n"
            "subjectKeyIdentifier = hash\n"
            "authorityKeyIdentifier = keyid:always,issuer\n"
            "[ ca ]\n"
            "default_ca = local_ca\n"
            "[ local_ca ]\n"
            f"database = {ca_database.as_posix()}\n"
            f"serial = {ca_serial.as_posix()}\n"
            f"new_certs_dir = {new_certificates.as_posix()}\n"
            f"certificate = {ca_cert.as_posix()}\n"
            f"private_key = {ca_key.as_posix()}\n"
            "default_days = 30\n"
            "default_md = sha256\n"
            "policy = local_policy\n"
            "x509_extensions = server_cert\n"
            "unique_subject = no\n"
            "[ local_policy ]\n"
            "commonName = supplied\n"
            "[ server_cert ]\n"
            "basicConstraints = critical,CA:FALSE\n"
            "keyUsage = critical,digitalSignature,keyEncipherment\n"
            "extendedKeyUsage = serverAuth\n"
            "subjectKeyIdentifier = hash\n"
            "authorityKeyIdentifier = keyid:always,issuer\n"
            "subjectAltName = DNS:localhost,IP:127.0.0.1,IP:::1\n",
            encoding="utf-8",
        )
        _openssl([
            "req", "-x509", "-newkey", "rsa:3072", "-nodes", "-days", "30", "-sha256",
            "-config", str(openssl_config),
            "-keyout", str(ca_key), "-out", str(ca_cert),
        ])
        _openssl(["req", "-newkey", "rsa:2048", "-nodes", "-sha256", "-subj", "/CN=localhost", "-keyout", str(leaf_key), "-out", str(leaf_csr)])
        _openssl([
            "ca", "-batch", "-notext", "-config", str(openssl_config),
            "-in", str(leaf_csr), "-out", str(leaf_cert),
        ])
        _ensure_certificate_chain(leaf_cert, ca_cert)
        for source, target in ((ca_cert, paths["ca_cert"]), (ca_key, paths["ca_key"]), (leaf_cert, paths["leaf_cert"]), (leaf_key, paths["leaf_key"])):
            os.replace(source, target)
    try:
        paths["ca_key"].chmod(0o600)
        paths["leaf_key"].chmod(0o600)
        paths["ca_cert"].chmod(0o644)
        paths["leaf_cert"].chmod(0o644)
    except OSError:
        # Windows ACLs, rather than POSIX mode bits, control these files.
        pass
    return paths


def _ensure_certificate_chain(leaf_cert: Path, ca_cert: Path) -> None:
    """Append the local CA so TLS servers present a complete chain on every OS."""
    leaf_data = leaf_cert.read_bytes()
    ca_data = ca_cert.read_bytes()
    if ca_data.strip() not in leaf_data:
        leaf_cert.write_bytes(leaf_data.rstrip() + b"\n" + ca_data)


def _openssl(arguments: list[str]) -> None:
    try:
        result = subprocess.run(
            ["openssl", *arguments], capture_output=True, text=True, check=False, timeout=30,
        )
    except OSError as error:
        raise RuntimeError("openssl is required for explicit TLS diagnostics") from error
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "openssl failed")


def _fingerprint(flow: NetworkFlowRecord) -> str:
    value = "|".join(
        str(item) for item in (
            flow.started_at, flow.ended_at, flow.mode, flow.destination_host,
            flow.destination_port, flow.http_status, flow.request_bytes, flow.response_bytes,
        )
    )
    return hashlib.sha256(value.encode()).hexdigest()


def _iso(value: datetime) -> str:
    return value.isoformat().replace("+00:00", "Z")
