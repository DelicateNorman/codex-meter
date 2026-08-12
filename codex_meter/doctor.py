from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path

from .storage import Storage


@dataclass(frozen=True, slots=True)
class Check:
    name: str
    status: str
    detail: str = ""


def run_doctor(codex_home: Path, storage: Storage) -> list[Check]:
    checks: list[Check] = []
    codex = shutil.which("codex")
    version = _command([codex, "--version"]) if codex else None
    checks.append(Check("Codex version", "yes" if version else "no", version or "not found"))

    sessions_dir = codex_home / "sessions"
    rollouts = list(sessions_dir.rglob("rollout-*.jsonl")) if sessions_dir.exists() else []
    checks.append(Check("Session JSONL", "yes" if rollouts else "no", f"{len(rollouts)} file(s)"))
    schema_capabilities = _latest_rollout_capabilities(rollouts)
    for name, capability in (
        ("Reasoning usage", "reasoning_output_tokens"),
        ("Cached input", "cached_input_tokens"),
        ("Cache write", "cache_write_input_tokens"),
        ("Turn timings", "time_to_first_token_ms"),
    ):
        checks.append(Check(name, "yes" if capability in schema_capabilities else "unknown"))

    config = _read_toml(codex_home / "config.toml")
    otel = config.get("otel") if isinstance(config.get("otel"), dict) else {}
    exporter = otel.get("exporter", "none") if isinstance(otel, dict) else "none"
    exporter_name = _otel_exporter_name(exporter)
    checks.append(Check("OpenTelemetry", "yes" if exporter_name != "none" else "disabled", exporter_name))

    app_server = bool(codex and _command([codex, "app-server", "--help"]))
    checks.append(Check("App Server", "yes" if app_server else "no", "experimental CLI" if app_server else ""))
    raw_response = _app_server_has_raw_response(codex) if app_server and codex else False
    checks.append(Check("Raw response events", "experimental" if raw_response else "unknown"))
    checks.append(Check("OTLP HTTP JSON collector", "yes", "logs + metrics + traces"))
    tcpdump = shutil.which("tcpdump")
    checks.append(Check("Passive packet metadata", "yes" if tcpdump else "no", tcpdump or "tcpdump not found"))
    checks.append(Check("CONNECT/reverse proxy", "yes", "content-free persistence"))
    openssl = shutil.which("openssl")
    checks.append(Check("TLS diagnostic", "disabled" if openssl else "no", "explicit opt-in" if openssl else "openssl not found"))
    checks.append(Check("SQLite", "yes" if storage.integrity_check() == "ok" else "no", storage.integrity_check()))
    return checks


def _latest_rollout_capabilities(paths: list[Path]) -> set[str]:
    capabilities: set[str] = set()
    for path in sorted(paths, reverse=True)[:10]:
        try:
            with path.open("r", encoding="utf-8", errors="ignore") as handle:
                for line in handle:
                    if '"token_count"' in line:
                        for key in (
                            "reasoning_output_tokens",
                            "cached_input_tokens",
                            "cache_write_input_tokens",
                            "time_to_first_token_ms",
                        ):
                            if key in line:
                                capabilities.add(key)
                    if "time_to_first_token_ms" in line:
                        capabilities.add("time_to_first_token_ms")
        except OSError:
            continue
        if len(capabilities) == 4:
            break
    return capabilities


def _app_server_has_raw_response(codex: str) -> bool:
    try:
        with tempfile.TemporaryDirectory(prefix="codex-meter-schema-") as directory:
            result = subprocess.run(
                [codex, "app-server", "generate-json-schema", "--experimental", "--out", directory],
                capture_output=True,
                text=True,
                timeout=20,
                check=False,
            )
            if result.returncode:
                return False
            schema = Path(directory) / "v2" / "RawResponseCompletedNotification.json"
            if not schema.exists():
                return False
            data = json.loads(schema.read_text(encoding="utf-8"))
            return data.get("title") == "RawResponseCompletedNotification"
    except (OSError, subprocess.SubprocessError, json.JSONDecodeError):
        return False


def _read_toml(path: Path) -> dict[str, object]:
    try:
        with path.open("rb") as handle:
            return tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError):
        return {}


def _otel_exporter_name(value: object) -> str:
    # Never stringify the exporter object: it can contain authorization headers.
    if isinstance(value, str):
        return value
    if isinstance(value, dict):
        if "otlp-http" in value:
            return "otlp-http"
        if "otlp-grpc" in value:
            return "otlp-grpc"
        return "configured"
    return "none"


def _command(command: list[str | None]) -> str | None:
    if any(part is None for part in command):
        return None
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=10, check=False)  # type: ignore[arg-type]
    except (OSError, subprocess.SubprocessError):
        return None
    return result.stdout.strip() if result.returncode == 0 else None
