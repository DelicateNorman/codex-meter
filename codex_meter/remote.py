"""Privacy-preserving collection from Codex sessions on SSH hosts."""

from __future__ import annotations

import codecs
import shlex
import subprocess
import tarfile
from dataclasses import dataclass
from pathlib import PurePosixPath
from typing import Sequence

from .collectors.session_jsonl import SessionJsonlCollector
from .config import validate_remote_host
from .pricing import PricingCatalog
from .storage import Storage


SSH_COMMAND = (
    "ssh",
    "-o", "BatchMode=yes",
    "-o", "ConnectTimeout=8",
    "-o", "ServerAliveInterval=10",
    "-o", "ServerAliveCountMax=3",
)
_REMOTE_ROOT = "$HOME/.codex/sessions"
_LIST_SCRIPT = r'''
set -eu
root="$HOME/.codex/sessions"
if [ ! -d "$root" ]; then
    echo "Codex session directory not found: $root" >&2
    exit 3
fi
find "$root" -type f -name 'rollout-*.jsonl' -print | while IFS= read -r file; do
    size=$(wc -c < "$file" | tr -d '[:space:]')
    if mtime=$(stat -c %Y "$file" 2>/dev/null); then
        :
    elif mtime=$(stat -f %m "$file" 2>/dev/null); then
        :
    else
        mtime=0
    fi
    relative=${file#"$root"/}
    printf '%s\t%s\t%s\n' "$size" "$mtime" "$relative"
done
'''.strip()


class RemoteError(RuntimeError):
    """An actionable SSH collection failure."""


@dataclass(frozen=True, slots=True)
class RemoteFile:
    host: str
    relative_path: str
    size_bytes: int
    mtime_ns: int

    @property
    def source_path(self) -> str:
        return f"ssh://{self.host}/~/.codex/sessions/{self.relative_path}"


@dataclass(slots=True)
class RemoteSyncResult:
    host: str
    discovered_files: int = 0
    imported_files: int = 0
    skipped_files: int = 0
    failed_files: int = 0
    inserted_turns: int = 0
    inserted_calls: int = 0
    inserted_tools: int = 0


def list_remote_rollouts(
    host: str,
    *,
    ssh_command: Sequence[str] = SSH_COMMAND,
    timeout: float = 20.0,
) -> list[RemoteFile]:
    """List remote rollout metadata without downloading conversation content."""

    alias = validate_remote_host(host)
    command = "sh -c " + shlex.quote(_LIST_SCRIPT)
    try:
        completed = subprocess.run(
            [*ssh_command, alias, command],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            check=False,
        )
    except FileNotFoundError as error:
        raise RemoteError("OpenSSH client was not found; install `ssh` first") from error
    except subprocess.TimeoutExpired as error:
        raise RemoteError(f"SSH connection to {alias} timed out") from error
    if completed.returncode != 0:
        detail = _short_error(completed.stderr)
        raise RemoteError(f"{alias}: {detail or 'remote session discovery failed'}")

    files: list[RemoteFile] = []
    for line in completed.stdout.splitlines():
        fields = line.split("\t", 2)
        if len(fields) != 3:
            continue
        try:
            size = int(fields[0])
            mtime_ns = int(fields[1]) * 1_000_000_000
            relative = _safe_relative_path(fields[2])
        except (ValueError, RemoteError):
            continue
        files.append(RemoteFile(alias, relative, size, mtime_ns))
    return sorted(files, key=lambda item: item.relative_path)


def sync_remote_rollouts(
    storage: Storage,
    catalog: PricingCatalog,
    host: str,
    *,
    force: bool = False,
    ssh_command: Sequence[str] = SSH_COMMAND,
) -> RemoteSyncResult:
    """Incrementally import one SSH host while keeping raw JSONL off local disk."""

    files = list_remote_rollouts(host, ssh_command=ssh_command)
    result = RemoteSyncResult(host=validate_remote_host(host), discovered_files=len(files))
    changed: list[RemoteFile] = []
    for remote_file in files:
        if not force and storage.source_is_current(
            remote_file.source_path, remote_file.size_bytes, remote_file.mtime_ns
        ):
            result.skipped_files += 1
        else:
            changed.append(remote_file)

    collector = SessionJsonlCollector(catalog)
    for offset in range(0, len(changed), 64):
        _import_batch(
            storage,
            collector,
            changed[offset : offset + 64],
            result,
            ssh_command=ssh_command,
        )
    return result


def _import_batch(
    storage: Storage,
    collector: SessionJsonlCollector,
    files: list[RemoteFile],
    result: RemoteSyncResult,
    *,
    ssh_command: Sequence[str],
) -> None:
    if not files:
        return
    host = files[0].host
    requested = {item.relative_path: item for item in files}
    path_arguments = " ".join(shlex.quote(item.relative_path) for item in files)
    command = f'tar -C "{_REMOTE_ROOT}" -cf - {path_arguments}'
    try:
        process = subprocess.Popen(
            [*ssh_command, host, command],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except FileNotFoundError as error:
        raise RemoteError("OpenSSH client was not found; install `ssh` first") from error

    seen: set[str] = set()
    try:
        assert process.stdout is not None
        with tarfile.open(fileobj=process.stdout, mode="r|*") as archive:
            for member in archive:
                if not member.isfile():
                    continue
                try:
                    relative = _safe_relative_path(member.name)
                except RemoteError:
                    continue
                remote_file = requested.get(relative)
                if remote_file is None:
                    continue
                extracted = archive.extractfile(member)
                if extracted is None:
                    result.failed_files += 1
                    seen.add(relative)
                    continue
                seen.add(relative)
                try:
                    with codecs.getreader("utf-8")(extracted, errors="replace") as stream:
                        parsed = collector.collect_stream(stream, source_path=remote_file.source_path)
                    turns, calls, tools = storage.import_session(
                        parsed,
                        source_path=remote_file.source_path,
                        size_bytes=member.size,
                        mtime_ns=int(member.mtime) * 1_000_000_000,
                    )
                except (OSError, ValueError):
                    result.failed_files += 1
                    continue
                result.imported_files += 1
                result.inserted_turns += turns
                result.inserted_calls += calls
                result.inserted_tools += tools
        process.stdout.close()
        return_code = process.wait(timeout=30)
    except (tarfile.TarError, OSError, subprocess.TimeoutExpired) as error:
        process.kill()
        process.wait()
        raise RemoteError(f"{host}: failed while streaming remote Rollouts: {error}") from error

    assert process.stderr is not None
    detail = _short_error(process.stderr.read().decode("utf-8", errors="replace"))
    process.stderr.close()
    if return_code != 0:
        raise RemoteError(f"{host}: {detail or 'remote Rollout transfer failed'}")
    result.failed_files += len(requested) - len(seen)


def _safe_relative_path(value: str) -> str:
    normalized = value[2:] if value.startswith("./") else value
    path = PurePosixPath(normalized)
    if (
        not normalized
        or path.is_absolute()
        or ".." in path.parts
        or any(character in normalized for character in ("\x00", "\n", "\r", "\t"))
        or not path.name.startswith("rollout-")
        or path.suffix != ".jsonl"
    ):
        raise RemoteError("unsafe remote Rollout path")
    return str(path)


def _short_error(value: str) -> str:
    return " ".join(value.strip().split())[:300]
