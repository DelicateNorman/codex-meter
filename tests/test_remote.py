from __future__ import annotations

import io
import json
import os
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from codex_meter.collectors.session_jsonl import SessionJsonlCollector
from codex_meter.config import initialize_home, load_remote_hosts, update_remote_hosts, validate_remote_host
from codex_meter.pricing import PricingCatalog
from codex_meter.remote import RemoteFile, RemoteSyncResult, _LIST_SCRIPT, _import_batch
from codex_meter.storage import Storage

from .helpers import base_events, complete_event, token_event


class _TarProcess:
    def __init__(self, payload: bytes) -> None:
        self.stdout = io.BytesIO(payload)
        self.stderr = io.BytesIO()

    def wait(self, timeout: float | None = None) -> int:
        return 0

    def kill(self) -> None:
        pass


class RemoteSourceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def test_remote_hosts_round_trip_without_touching_other_config(self) -> None:
        home = self.root / "meter"
        initialize_home(home)
        updated = update_remote_hosts(home, ["devbox", "gpu-box", "devbox"])
        self.assertEqual(updated, ("devbox", "gpu-box"))
        self.assertEqual(load_remote_hosts(home), updated)
        self.assertIn("[privacy]", (home / "config.toml").read_text(encoding="utf-8"))

    def test_remote_host_rejects_ssh_command_syntax(self) -> None:
        self.assertEqual(validate_remote_host("dev.example"), "dev.example")
        for unsafe in ("-oProxyCommand=x", "dev box", "user@host", "host;touch-x"):
            with self.assertRaises(ValueError):
                validate_remote_host(unsafe)

    @unittest.skipIf(os.name == "nt", "remote discovery script runs on POSIX SSH hosts")
    def test_discovery_script_is_portable_on_posix_shell(self) -> None:
        rollout = self.root / ".codex" / "sessions" / "2026" / "08" / "rollout-one.jsonl"
        rollout.parent.mkdir(parents=True)
        rollout.write_text("{}\n", encoding="utf-8")
        environment = os.environ.copy()
        environment["HOME"] = str(self.root)
        completed = subprocess.run(
            ["sh", "-c", _LIST_SCRIPT],
            text=True,
            capture_output=True,
            env=environment,
            check=False,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        fields = completed.stdout.strip().split("\t", 2)
        self.assertEqual(fields[0], "3")
        self.assertEqual(fields[2], "2026/08/rollout-one.jsonl")

    def test_tar_stream_imports_metrics_without_saving_raw_rollout(self) -> None:
        secret = "REMOTE-PROMPT-MUST-NOT-BE-STORED"
        events = base_events() + [
            {
                "timestamp": "2026-08-12T00:00:03Z",
                "type": "event_msg",
                "payload": {"type": "user_message", "message": secret},
            },
            token_event("2026-08-12T00:00:04Z", (100, 80, 0, 10, 4, 110)),
            complete_event(),
        ]
        payload = ("\n".join(json.dumps(item) for item in events) + "\n").encode()
        relative = "2026/08/12/rollout-test.jsonl"
        archive_buffer = io.BytesIO()
        with tarfile.open(fileobj=archive_buffer, mode="w") as archive:
            info = tarfile.TarInfo(relative)
            info.size = len(payload)
            info.mtime = 1_786_492_800
            archive.addfile(info, io.BytesIO(payload))

        remote_file = RemoteFile("devbox", relative, len(payload), info.mtime * 1_000_000_000)
        result = RemoteSyncResult("devbox", discovered_files=1)
        database = self.root / "meter.db"
        with Storage(database) as storage:
            storage.migrate()
            catalog = PricingCatalog.bundled()
            storage.sync_pricing(catalog)
            with patch(
                "codex_meter.remote.subprocess.Popen",
                return_value=_TarProcess(archive_buffer.getvalue()),
            ):
                _import_batch(
                    storage,
                    SessionJsonlCollector(catalog),
                    [remote_file],
                    result,
                    ssh_command=("ssh",),
                )
            overview = storage.overview("2026-08-12")
            source = storage.connection.execute(
                "SELECT source_path FROM import_files"
            ).fetchone()[0]
            self.assertTrue(storage.source_is_current(
                remote_file.source_path, len(payload), info.mtime * 1_000_000_000,
            ))

        self.assertEqual(result.imported_files, 1)
        self.assertEqual(result.inserted_calls, 1)
        self.assertEqual(overview["total_tokens"], 110)
        self.assertEqual(source, remote_file.source_path)
        for local_file in self.root.rglob("*"):
            if local_file.is_file():
                self.assertNotIn(secret.encode(), local_file.read_bytes(), str(local_file))


if __name__ == "__main__":
    unittest.main()
