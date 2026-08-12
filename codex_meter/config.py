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
