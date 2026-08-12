"""Network diagnostics that persist metadata and timing, never packet payloads."""

from __future__ import annotations

import hashlib
import json
import re
import selectors
import shutil
import socket
import socketserver
import ssl
import subprocess
import time
from dataclasses import asdict
from datetime import datetime, timezone
from typing import Iterable

from .models import NetworkFlowRecord, Quality
from .storage import Storage


PACKET_LINE = re.compile(
    r"^\s*(?P<ts>\d+(?:\.\d+)?)\s+IP6?\s+(?P<src>\S+)\s+>\s+(?P<dst>\S+):.*?length\s+(?P<length>\d+)"
)


def probe_endpoint(host: str, port: int = 443, timeout: float = 10.0) -> NetworkFlowRecord:
    started_wall = datetime.now(timezone.utc)
    started = time.monotonic()
    destination_ip = tls_version = alpn = error_type = None
    dns_ms = tcp_ms = tls_ms = None
    success = False
    raw: socket.socket | None = None
    wrapped: ssl.SSLSocket | None = None
    try:
        mark = time.monotonic()
        addresses = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
        dns_ms = (time.monotonic() - mark) * 1000
        if not addresses:
            raise OSError("no addresses")
        family, socktype, proto, _, sockaddr = addresses[0]
        destination_ip = str(sockaddr[0])
        raw = socket.socket(family, socktype, proto)
        raw.settimeout(timeout)
        mark = time.monotonic()
        raw.connect(sockaddr)
        tcp_ms = (time.monotonic() - mark) * 1000
        context = ssl.create_default_context()
        context.set_alpn_protocols(["h2", "http/1.1"])
        mark = time.monotonic()
        wrapped = context.wrap_socket(raw, server_hostname=host)
        raw = None
        tls_ms = (time.monotonic() - mark) * 1000
        tls_version = wrapped.version()
        alpn = wrapped.selected_alpn_protocol()
        success = True
    except (OSError, ssl.SSLError) as error:
        error_type = type(error).__name__
    finally:
        if wrapped is not None:
            wrapped.close()
        if raw is not None:
            raw.close()
    ended = datetime.now(timezone.utc)
    flow = NetworkFlowRecord(
        event_fingerprint="",
        mode="probe",
        data_source="socket_probe",
        started_at=_iso(started_wall),
        ended_at=_iso(ended),
        destination_host=host,
        destination_ip=destination_ip,
        destination_port=port,
        protocol="tls",
        tls_version=tls_version,
        alpn=alpn,
        dns_ms=dns_ms,
        tcp_ms=tcp_ms,
        tls_ms=tls_ms,
        duration_ms=(time.monotonic() - started) * 1000,
        success=success,
        error_type=error_type,
        quality=Quality("socket_probe"),
    )
    flow.event_fingerprint = _flow_fingerprint(flow)
    return flow


def capture_metadata(
    hosts: list[str], *, interface: str = "any", port: int = 443,
    duration: float = 15.0, packet_limit: int = 5000,
) -> tuple[list[NetworkFlowRecord], str | None]:
    """Run tcpdump without -X/-A/-w and aggregate only packet direction and length."""
    tcpdump = shutil.which("tcpdump")
    if not tcpdump:
        return [], "tcpdump not found"
    resolved: dict[str, str] = {}
    for host in hosts:
        try:
            for info in socket.getaddrinfo(host, port, type=socket.SOCK_STREAM):
                resolved[str(info[4][0])] = host
        except OSError:
            continue
    if not resolved:
        return [], "no capture host resolved"
    host_filter = " or ".join(f"host {ip}" for ip in resolved)
    command = [
        tcpdump, "-i", interface, "-nn", "-tt", "-l", "-q", "-c", str(max(1, packet_limit)),
        f"tcp port {int(port)} and ({host_filter})",
    ]
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        stdout, stderr = process.communicate(timeout=max(0.1, duration))
    except subprocess.TimeoutExpired:
        process.terminate()
        try:
            stdout, stderr = process.communicate(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            stdout, stderr = process.communicate()
    flows = parse_tcpdump(stdout.splitlines(), resolved, port)
    error = None
    if process.returncode not in (0, -15) and not flows:
        error = _safe_tcpdump_error(stderr)
    return flows, error


def parse_tcpdump(lines: Iterable[str], remote_ips: dict[str, str], port: int = 443) -> list[NetworkFlowRecord]:
    groups: dict[str, dict[str, float | int]] = {}
    for line in lines:
        match = PACKET_LINE.match(line)
        if not match:
            continue
        src_ip, src_port = _split_endpoint(match.group("src"))
        dst_ip, dst_port = _split_endpoint(match.group("dst"))
        length = int(match.group("length"))
        timestamp = float(match.group("ts"))
        remote_ip = src_ip if src_ip in remote_ips else dst_ip if dst_ip in remote_ips else None
        if remote_ip is None:
            continue
        bucket = groups.setdefault(
            remote_ip,
            {"first": timestamp, "last": timestamp, "out_packets": 0, "in_packets": 0, "out_bytes": 0, "in_bytes": 0},
        )
        bucket["first"] = min(float(bucket["first"]), timestamp)
        bucket["last"] = max(float(bucket["last"]), timestamp)
        if dst_ip == remote_ip and (dst_port == port or src_port != port):
            bucket["out_packets"] = int(bucket["out_packets"]) + 1
            bucket["out_bytes"] = int(bucket["out_bytes"]) + length
        else:
            bucket["in_packets"] = int(bucket["in_packets"]) + 1
            bucket["in_bytes"] = int(bucket["in_bytes"]) + length
    output: list[NetworkFlowRecord] = []
    for remote_ip, bucket in groups.items():
        started = datetime.fromtimestamp(float(bucket["first"]), timezone.utc)
        ended = datetime.fromtimestamp(float(bucket["last"]), timezone.utc)
        flow = NetworkFlowRecord(
            event_fingerprint="",
            mode="passive",
            data_source="tcpdump_metadata",
            started_at=_iso(started),
            ended_at=_iso(ended),
            destination_host=remote_ips[remote_ip],
            destination_ip=remote_ip,
            destination_port=port,
            protocol="tcp/tls-opaque",
            request_bytes=int(bucket["out_bytes"]),
            response_bytes=int(bucket["in_bytes"]),
            packets_out=int(bucket["out_packets"]),
            packets_in=int(bucket["in_packets"]),
            duration_ms=(float(bucket["last"]) - float(bucket["first"])) * 1000,
            success=True,
            quality=Quality("tcpdump_metadata"),
        )
        flow.event_fingerprint = _flow_fingerprint(flow)
        output.append(flow)
    return output


class TunnelProxyServer(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, address: tuple[str, int], storage: Storage) -> None:
        self.storage = storage
        super().__init__(address, TunnelProxyHandler)


class TunnelProxyHandler(socketserver.BaseRequestHandler):
    server: TunnelProxyServer

    def handle(self) -> None:
        started_wall = datetime.now(timezone.utc)
        started = time.monotonic()
        request_bytes = response_bytes = 0
        remote: socket.socket | None = None
        host = destination_ip = error_type = None
        port = 443
        dns_ms = tcp_ms = None
        success = False
        try:
            header = _read_header(self.request)
            first_line = header.split(b"\r\n", 1)[0].decode("ascii", "replace")
            method, target, _version = first_line.split(" ", 2)
            if method.upper() != "CONNECT":
                self.request.sendall(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                return
            host, port = _parse_connect_target(target)
            mark = time.monotonic()
            addresses = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
            dns_ms = (time.monotonic() - mark) * 1000
            family, socktype, proto, _, sockaddr = addresses[0]
            destination_ip = str(sockaddr[0])
            remote = socket.socket(family, socktype, proto)
            remote.settimeout(15)
            mark = time.monotonic()
            remote.connect(sockaddr)
            tcp_ms = (time.monotonic() - mark) * 1000
            remote.settimeout(None)
            self.request.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            success = True
            request_bytes, response_bytes = _relay(self.request, remote)
        except (OSError, ValueError) as error:
            error_type = type(error).__name__
            try:
                self.request.sendall(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            except OSError:
                pass
        finally:
            if remote is not None:
                remote.close()
            ended = datetime.now(timezone.utc)
            flow = NetworkFlowRecord(
                event_fingerprint="", mode="tunnel_proxy", data_source="local_connect_proxy",
                started_at=_iso(started_wall), ended_at=_iso(ended), destination_host=host,
                destination_ip=destination_ip, destination_port=port, protocol="tls-opaque",
                request_bytes=request_bytes, response_bytes=response_bytes, dns_ms=dns_ms,
                tcp_ms=tcp_ms, duration_ms=(time.monotonic() - started) * 1000,
                success=success, error_type=error_type, quality=Quality("local_connect_proxy"),
            )
            flow.event_fingerprint = _flow_fingerprint(flow)
            self.server.storage.insert_network_flow(flow)


def _relay(client: socket.socket, remote: socket.socket) -> tuple[int, int]:
    selector = selectors.DefaultSelector()
    selector.register(client, selectors.EVENT_READ, (client, remote, "out"))
    selector.register(remote, selectors.EVENT_READ, (remote, client, "in"))
    totals = {"out": 0, "in": 0}
    while selector.get_map():
        events = selector.select(timeout=60)
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


def _read_header(sock: socket.socket) -> bytes:
    data = bytearray()
    while b"\r\n\r\n" not in data:
        chunk = sock.recv(4096)
        if not chunk:
            break
        data.extend(chunk)
        if len(data) > 65536:
            raise ValueError("proxy header too large")
    return bytes(data)


def _parse_connect_target(value: str) -> tuple[str, int]:
    if value.startswith("["):
        host, _, port = value[1:].partition("]:")
    else:
        host, separator, port = value.rpartition(":")
        if not separator:
            return value, 443
    return host, int(port)


def _split_endpoint(value: str) -> tuple[str, int | None]:
    cleaned = value.rstrip(":")
    host, separator, port = cleaned.rpartition(".")
    if separator and port.isdigit():
        return host, int(port)
    return cleaned, None


def _safe_tcpdump_error(value: str) -> str:
    lowered = value.lower()
    if "permission" in lowered or "operation not permitted" in lowered:
        return "tcpdump permission denied; grant capture capability or run this command with appropriate privileges"
    return value.strip().splitlines()[-1][:300] if value.strip() else "tcpdump failed"


def _flow_fingerprint(flow: NetworkFlowRecord) -> str:
    data = asdict(flow)
    data.pop("event_fingerprint", None)
    data["quality"] = flow.quality.source
    return hashlib.sha256(json.dumps(data, sort_keys=True, default=str, separators=(",", ":")).encode()).hexdigest()


def _iso(value: datetime) -> str:
    return value.isoformat().replace("+00:00", "Z")
