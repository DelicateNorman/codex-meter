from __future__ import annotations

import json
import os
import shutil
import socket
import socketserver
import ssl
import io
import subprocess
import tempfile
import threading
import unittest
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from codex_meter.collectors.app_server import AppServerAdapter, _pump
from codex_meter.collectors.otlp_http import OtlpServer, parse_logs, parse_metrics
from codex_meter.network import TunnelProxyServer, _default_capture_interface, parse_tcpdump
from codex_meter.pricing import PricingCatalog
from codex_meter.proxy import ReverseProxyServer, initialize_tls_material, wrap_server_tls
from codex_meter.storage import Storage


class LiveObservabilityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.storage = Storage(self.root / "meter.db")
        self.storage.migrate()
        self.catalog = PricingCatalog.bundled()
        self.storage.sync_pricing(self.catalog)

    def tearDown(self) -> None:
        self.storage.close()
        self.temp.cleanup()

    def test_otlp_parser_keeps_only_allowlisted_metadata(self) -> None:
        metric_doc = {
            "resourceMetrics": [{
                "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "SECRET SERVICE"}}]},
                "scopeMetrics": [{"metrics": [{
                    "name": "codex.turn.ttfm.duration_ms",
                    "histogram": {"dataPoints": [{
                        "startTimeUnixNano": "1786492800000000000",
                        "timeUnixNano": "1786492801000000000",
                        "count": "2", "sum": 3000, "min": 1000, "max": 2000,
                        "explicitBounds": [1000, 2000], "bucketCounts": [0, 1, 1],
                        "attributes": [
                            {"key": "thread.id", "value": {"stringValue": "thread-live"}},
                            {"key": "turn.id", "value": {"stringValue": "turn-live"}},
                            {"key": "authorization", "value": {"stringValue": "Bearer TOP-SECRET"}},
                        ],
                    }]},
                }]}],
            }]
        }
        points = parse_metrics(metric_doc)
        self.assertEqual(len(points), 1)
        self.assertEqual(points[0].turn_id, "turn-live")
        self.assertNotIn("authorization", points[0].attributes)
        self.storage.insert_metric_points(points)

        log_doc = {"resourceLogs": [{"scopeLogs": [{"logRecords": [{
            "timeUnixNano": "1786492801000000000",
            "severityText": "INFO",
            "body": {"stringValue": "TOP SECRET PROMPT"},
            "attributes": [
                {"key": "event.name", "value": {"stringValue": "codex.api_request"}},
                {"key": "duration_ms", "value": {"intValue": "120"}},
                {"key": "arguments", "value": {"stringValue": "TOP SECRET ARGUMENTS"}},
            ],
        }]}]}]}
        records = parse_logs(log_doc)
        self.assertEqual(records[0].event_name, "codex.api_request")
        self.storage.insert_telemetry_logs(records)
        self._assert_database_excludes("TOP SECRET", "Bearer")

    def test_real_otlp_http_endpoint_and_histogram_analysis_source(self) -> None:
        server = OtlpServer(("127.0.0.1", 0), self.storage)
        thread = _serve(server)
        payload = {
            "resourceMetrics": [{"scopeMetrics": [{"metrics": [{
                "name": "codex.api_request.duration_ms",
                "histogram": {"dataPoints": [{
                    "timeUnixNano": "1786492801000000000", "count": "1", "sum": 250,
                    "explicitBounds": [100, 250, 500], "bucketCounts": [0, 0, 1, 0],
                }]},
            }]}]}]
        }
        request = urllib.request.Request(
            f"http://127.0.0.1:{server.server_address[1]}/v1/metrics",
            data=json.dumps(payload).encode(), headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=3) as response:
            self.assertEqual(response.status, 200)
        server.shutdown()
        thread.join(timeout=3)
        server.server_close()
        self.assertEqual(self.storage.counts()["metric_points"], 1)

    def test_app_server_adapter_extracts_usage_tools_and_compaction_without_content(self) -> None:
        adapter = AppServerAdapter(self.storage, self.catalog)
        messages = [
            {"method": "thread/started", "emittedAtMs": 1786492800000, "params": {"thread": {"id": "thread-live", "cwd": "/work/live"}}},
            {"method": "turn/started", "emittedAtMs": 1786492801000, "params": {"threadId": "thread-live", "turn": {"id": "turn-live", "status": "inProgress"}}},
            {"method": "item/started", "emittedAtMs": 1786492801500, "params": {"threadId": "thread-live", "turnId": "turn-live", "item": {"id": "tool-live", "type": "commandExecution", "command": "TOP SECRET COMMAND"}}},
            {"method": "item/agentMessage/delta", "emittedAtMs": 1786492802000, "params": {"threadId": "thread-live", "turnId": "turn-live", "delta": "TOP SECRET RESPONSE"}},
            {"method": "item/completed", "emittedAtMs": 1786492802500, "params": {"threadId": "thread-live", "turnId": "turn-live", "item": {"id": "tool-live", "type": "commandExecution", "command": "TOP SECRET COMMAND", "aggregatedOutput": "TOP SECRET OUTPUT", "status": "completed", "durationMs": 1000, "exitCode": 0}}},
            {"method": "rawResponse/completed", "emittedAtMs": 1786492803000, "params": {"threadId": "thread-live", "turnId": "turn-live", "responseId": "resp-live", "usage": {"inputTokens": 100, "cachedInputTokens": 80, "outputTokens": 20, "reasoningOutputTokens": 10, "totalTokens": 120}}},
            {"method": "thread/compacted", "emittedAtMs": 1786492803500, "params": {"threadId": "thread-live", "turnId": "turn-live"}},
            {"method": "turn/completed", "emittedAtMs": 1786492804000, "params": {"threadId": "thread-live", "turn": {"id": "turn-live", "status": "completed"}}},
        ]
        self.assertGreater(sum(adapter.ingest(message) for message in messages), 0)
        counts = self.storage.counts()
        self.assertEqual(counts["llm_calls"], 1)
        self.assertEqual(counts["tool_calls"], 1)
        self.assertEqual(counts["compactions"], 1)
        call = self.storage.usage_calls()[0]
        self.assertEqual(call["response_id"], "resp-live")
        self.assertEqual(call["cached_input_tokens"], 80)
        turn, _, _ = self.storage.turn_waterfall("turn-live")
        self.assertEqual(turn["ttfm_ms"], 1000)
        self._assert_database_excludes("TOP SECRET")

    def test_app_server_proxy_enables_exact_raw_events_in_memory_only(self) -> None:
        adapter = AppServerAdapter(self.storage, self.catalog)
        source = io.BytesIO(
            json.dumps({
                "method": "thread/start", "id": 9,
                "params": {"cwd": "/work", "input": "TOP SECRET PROMPT"},
            }).encode() + b"\n"
        )
        target = io.BytesIO()
        _pump(source, target, adapter, "client")
        forwarded = json.loads(target.getvalue())
        self.assertTrue(forwarded["params"]["experimentalRawEvents"])
        self.assertEqual(forwarded["params"]["input"], "TOP SECRET PROMPT")
        self._assert_database_excludes("TOP SECRET")

    def test_tcpdump_parser_aggregates_direction_and_length_only(self) -> None:
        lines = [
            "1786492800.100000 IP 10.0.0.2.50000 > 203.0.113.8.443: tcp 200, length 200",
            "1786492800.300000 IP 203.0.113.8.443 > 10.0.0.2.50000: tcp 800, length 800",
        ]
        flows = parse_tcpdump(lines, {"203.0.113.8": "api.openai.com"})
        self.assertEqual(len(flows), 1)
        self.assertEqual(flows[0].request_bytes, 200)
        self.assertEqual(flows[0].response_bytes, 800)
        self.assertEqual(flows[0].packets_out, 1)
        self.assertEqual(flows[0].packets_in, 1)

    def test_capture_interface_prefers_platform_aggregate_devices(self) -> None:
        listing = "1.en0 [Up, Running]\n2.pktap [Up, Running]\n"
        with patch(
            "codex_meter.network.subprocess.run",
            return_value=SimpleNamespace(stdout=listing),
        ):
            self.assertEqual(_default_capture_interface("tcpdump"), "pktap")

    def test_connect_tunnel_proxies_bytes_and_saves_only_counts(self) -> None:
        echo = _EchoServer(("127.0.0.1", 0), _EchoHandler)
        echo_thread = _serve(echo)
        proxy = TunnelProxyServer(("127.0.0.1", 0), self.storage)
        proxy_thread = _serve(proxy)
        secret = b"TOP SECRET TUNNEL PAYLOAD"
        with socket.create_connection(proxy.server_address, timeout=3) as client:
            target = f"127.0.0.1:{echo.server_address[1]}"
            client.sendall(f"CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n".encode())
            self.assertIn(b"200", client.recv(4096))
            client.sendall(secret)
            self.assertEqual(client.recv(len(secret)), secret)
        proxy.shutdown()
        echo.shutdown()
        proxy_thread.join(timeout=3)
        echo_thread.join(timeout=3)
        proxy.server_close()
        echo.server_close()
        rows = self.storage.recent_network()
        self.assertEqual(rows[0]["mode"], "tunnel_proxy")
        self.assertGreaterEqual(rows[0]["request_bytes"], len(secret))
        self._assert_database_excludes("TOP SECRET")

    def test_reverse_proxy_streams_sse_but_does_not_store_bodies_or_headers(self) -> None:
        upstream = ThreadingHTTPServer(("127.0.0.1", 0), _SseHandler)
        upstream_thread = _serve(upstream)
        proxy = ReverseProxyServer(
            ("127.0.0.1", 0), self.storage,
            f"http://127.0.0.1:{upstream.server_address[1]}",
        )
        proxy_thread = _serve(proxy)
        secret = b"TOP SECRET REQUEST"
        request = urllib.request.Request(
            f"http://127.0.0.1:{proxy.server_address[1]}/v1/responses",
            data=secret,
            headers={"Authorization": "Bearer TOP-SECRET-KEY", "Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=3) as response:
            body = response.read()
        self.assertIn(b"TOP SECRET RESPONSE", body)
        proxy.shutdown()
        upstream.shutdown()
        proxy_thread.join(timeout=3)
        upstream_thread.join(timeout=3)
        proxy.server_close()
        upstream.server_close()
        row = self.storage.recent_network()[0]
        self.assertEqual(row["http_status"], 200)
        self.assertIsNotNone(row["first_event_ms"])
        self._assert_database_excludes("TOP SECRET", "TOP-SECRET-KEY")

    def test_reverse_proxy_relays_websocket_upgrade_as_opaque_bytes(self) -> None:
        upstream = socketserver.ThreadingTCPServer(("127.0.0.1", 0), _UpgradeEchoHandler)
        upstream.daemon_threads = True
        upstream_thread = _serve(upstream)
        proxy = ReverseProxyServer(
            ("127.0.0.1", 0), self.storage,
            f"http://127.0.0.1:{upstream.server_address[1]}",
        )
        proxy_thread = _serve(proxy)
        secret = b"TOP SECRET WS FRAME"
        with socket.create_connection(proxy.server_address, timeout=3) as client:
            client.sendall(
                b"GET /responses HTTP/1.1\r\nHost: localhost\r\n"
                b"Connection: Upgrade\r\nUpgrade: websocket\r\n"
                b"Sec-WebSocket-Key: test\r\nSec-WebSocket-Version: 13\r\n\r\n"
            )
            self.assertIn(b"101", client.recv(4096))
            client.sendall(secret)
            self.assertEqual(client.recv(len(secret)), secret)
        proxy.shutdown()
        upstream.shutdown()
        proxy_thread.join(timeout=3)
        upstream_thread.join(timeout=3)
        proxy.server_close()
        upstream.server_close()
        row = self.storage.recent_network()[0]
        self.assertEqual(row["mode"], "websocket_reverse_proxy")
        self.assertEqual(row["http_status"], 101)
        self._assert_database_excludes("TOP SECRET")

    @unittest.skipUnless(shutil.which("openssl"), "openssl is optional")
    def test_tls_material_is_private_and_reusable(self) -> None:
        paths = initialize_tls_material(self.root / "tls")
        self.assertTrue(paths["ca_cert"].exists())
        if os.name != "nt":
            self.assertEqual(paths["ca_key"].stat().st_mode & 0o777, 0o600)
        certificate = subprocess.run(
            ["openssl", "x509", "-in", str(paths["ca_cert"]), "-noout", "-text"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
        self.assertIn("CA:TRUE", certificate)
        self.assertIn("Certificate Sign", certificate)
        verification = subprocess.run(
            [
                "openssl", "verify", "-CAfile", str(paths["ca_cert"]),
                str(paths["leaf_cert"]),
            ],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
        self.assertIn("OK", verification)
        self.assertEqual(
            paths["leaf_cert"].read_text(encoding="utf-8").count("BEGIN CERTIFICATE"),
            2,
        )
        self.assertEqual(paths, initialize_tls_material(self.root / "tls"))

    @unittest.skipUnless(shutil.which("openssl"), "openssl is optional")
    def test_explicit_tls_reverse_proxy_terminates_and_reencrypts(self) -> None:
        upstream = ThreadingHTTPServer(("127.0.0.1", 0), _SseHandler)
        upstream_thread = _serve(upstream)
        paths = initialize_tls_material(self.root / "tls-live")
        proxy = ReverseProxyServer(
            ("127.0.0.1", 0), self.storage,
            f"http://127.0.0.1:{upstream.server_address[1]}",
        )
        wrap_server_tls(proxy, paths["leaf_cert"], paths["leaf_key"])
        proxy_thread = _serve(proxy)
        # Certificate construction and trust are validated separately above.
        # This test focuses on TLS termination, forwarding, and data minimization.
        context = ssl.create_default_context()
        context.check_hostname = False
        context.verify_mode = ssl.CERT_NONE
        opener = urllib.request.build_opener(
            urllib.request.ProxyHandler({}),
            urllib.request.HTTPSHandler(context=context),
        )
        request = urllib.request.Request(
            f"https://localhost:{proxy.server_address[1]}/v1/responses",
            data=b"TLS TOP SECRET REQUEST", headers={"Content-Type": "application/json"},
        )
        with opener.open(request, timeout=3) as response:
            self.assertIn(b"TOP SECRET RESPONSE", response.read())
        proxy.shutdown()
        upstream.shutdown()
        proxy_thread.join(timeout=3)
        upstream_thread.join(timeout=3)
        proxy.server_close()
        upstream.server_close()
        row = self.storage.recent_network()[0]
        self.assertEqual(row["mode"], "tls_reverse_proxy")
        self.assertEqual(row["http_status"], 200)
        self._assert_database_excludes("TOP SECRET")

    def _assert_database_excludes(self, *needles: str) -> None:
        data = b"".join(path.read_bytes() for path in self.root.glob("meter.db*"))
        for needle in needles:
            self.assertNotIn(needle.encode(), data)


class _EchoServer(ThreadingHTTPServer):
    pass


class _EchoHandler(BaseHTTPRequestHandler):
    def handle(self) -> None:
        while True:
            data = self.request.recv(65536)
            if not data:
                return
            self.request.sendall(data)


class _SseHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("Content-Length", "0"))
        self.rfile.read(length)
        body = b"event: response.output_text.delta\ndata: TOP SECRET RESPONSE\n\n"
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *_args: object) -> None:
        pass


class _UpgradeEchoHandler(socketserver.BaseRequestHandler):
    def handle(self) -> None:
        data = bytearray()
        while b"\r\n\r\n" not in data:
            chunk = self.request.recv(4096)
            if not chunk:
                return
            data.extend(chunk)
        self.request.sendall(
            b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n"
        )
        while True:
            chunk = self.request.recv(65536)
            if not chunk:
                return
            self.request.sendall(chunk)


def _serve(server: object) -> threading.Thread:
    thread = threading.Thread(target=server.serve_forever, daemon=True)  # type: ignore[attr-defined]
    thread.start()
    return thread


if __name__ == "__main__":
    unittest.main()
