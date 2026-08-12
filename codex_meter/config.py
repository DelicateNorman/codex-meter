from __future__ import annotations

import getpass
import json
import os
import re
import shutil
import tomllib
from dataclasses import dataclass
from importlib.resources import files
from pathlib import Path


DEFAULT_CONFIG = """# Codex Meter local-first configuration
[privacy]
store_prompt = false
store_response = false
store_tool_output = false
store_headers = false
diagnostic_payload_logging = false

[retention]
raw_days = 7
call_days = 90

[collector]
batch_size = 500
fail_open = true

[network]
store_payloads = false
store_headers = false
passive_capture = false
tls_diagnostic = false

[identity]
account_tracking = false
account_label = ""

[remotes]
hosts = []
"""


@dataclass(frozen=True, slots=True)
class LocalIdentity:
    """Non-secret ownership metadata for one local meter process."""

    uid: int | None
    username: str
    account_tracking: bool = False
    account_label: str | None = None


def load_identity(home: Path) -> LocalIdentity:
    uid = os.getuid() if hasattr(os, "getuid") else None
    username = getpass.getuser() or "unknown"
    tracking = False
    label: str | None = None
    config = home / "config.toml"
    if config.exists():
        try:
            parsed = tomllib.loads(config.read_text(encoding="utf-8"))
            section = parsed.get("identity", {})
            tracking = bool(section.get("account_tracking", False))
            configured = section.get("account_label")
            if isinstance(configured, str) and configured.strip():
                label = configured.strip()
        except (OSError, tomllib.TOMLDecodeError):
            # Collection must remain available when a hand-edited optional
            # config section is malformed. `account status` exposes the result.
            pass
    return LocalIdentity(uid, username, tracking, label if tracking else None)


def update_account_identity(home: Path, *, enabled: bool, label: str | None) -> LocalIdentity:
    """Update only the non-secret [identity] section in config.toml."""

    initialize_home(home)
    cleaned = (label or "").strip()
    if any(character in cleaned for character in ("\n", "\r", "\x00")):
        raise ValueError("account label must be a single line")
    config = home / "config.toml"
    text = config.read_text(encoding="utf-8")
    block = (
        "[identity]\n"
        f"account_tracking = {'true' if enabled else 'false'}\n"
        f"account_label = {json.dumps(cleaned, ensure_ascii=False)}\n"
    )
    pattern = re.compile(r"(?ms)^\[identity\]\s*\n.*?(?=^\[[^\n]+\]\s*$|\Z)")
    if pattern.search(text):
        text = pattern.sub(block + "\n", text, count=1)
    else:
        text = text.rstrip() + "\n\n" + block
    config.write_text(text, encoding="utf-8")
    return load_identity(home)


_REMOTE_HOST = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")


def validate_remote_host(host: str) -> str:
    """Validate one OpenSSH config alias without accepting command syntax."""

    cleaned = host.strip()
    if not _REMOTE_HOST.fullmatch(cleaned):
        raise ValueError(
            "remote host must be an SSH config alias containing only letters, "
            "numbers, dots, underscores, and hyphens"
        )
    return cleaned


def load_remote_hosts(home: Path) -> tuple[str, ...]:
    config = home / "config.toml"
    if not config.exists():
        return ()
    try:
        parsed = tomllib.loads(config.read_text(encoding="utf-8"))
        configured = parsed.get("remotes", {}).get("hosts", [])
    except (OSError, AttributeError, tomllib.TOMLDecodeError):
        return ()
    if not isinstance(configured, list):
        return ()
    hosts: list[str] = []
    for item in configured:
        if not isinstance(item, str):
            continue
        try:
            host = validate_remote_host(item)
        except ValueError:
            continue
        if host not in hosts:
            hosts.append(host)
    return tuple(hosts)


def update_remote_hosts(home: Path, hosts: list[str] | tuple[str, ...]) -> tuple[str, ...]:
    """Replace the configured SSH aliases while preserving all other config."""

    initialize_home(home)
    normalized: list[str] = []
    for item in hosts:
        host = validate_remote_host(item)
        if host not in normalized:
            normalized.append(host)
    config = home / "config.toml"
    text = config.read_text(encoding="utf-8")
    block = "[remotes]\nhosts = " + json.dumps(normalized, ensure_ascii=False) + "\n"
    pattern = re.compile(r"(?ms)^\[remotes\]\s*\n.*?(?=^\[[^\n]+\]\s*$|\Z)")
    if pattern.search(text):
        text = pattern.sub(block + "\n", text, count=1)
    else:
        text = text.rstrip() + "\n\n" + block
    config.write_text(text, encoding="utf-8")
    return tuple(normalized)


def initialize_home(home: Path) -> None:
    home.mkdir(parents=True, exist_ok=True, mode=0o700)
    try:
        home.chmod(0o700)
    except OSError:
        pass
    (home / "logs").mkdir(exist_ok=True, mode=0o700)
    config = home / "config.toml"
    if not config.exists():
        config.write_text(DEFAULT_CONFIG, encoding="utf-8")
    pricing = home / "pricing.json"
    if not pricing.exists():
        bundled = Path(str(files("codex_meter").joinpath("data/pricing.json")))
        shutil.copyfile(bundled, pricing)
