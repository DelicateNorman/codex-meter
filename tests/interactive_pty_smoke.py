from __future__ import annotations

import argparse
import os
import select
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def wait_for(read_chunk, expected: str, timeout: float = 12.0) -> str:
    output = ""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            chunk = read_chunk()
        except (BlockingIOError, socket.timeout):
            chunk = ""
        except EOFError:
            break
        if chunk:
            output += chunk
            if expected in output:
                return output
        else:
            time.sleep(0.02)
    raise AssertionError(f"PTY output did not contain {expected!r}; tail={output[-1000:]!r}")


def run_posix(binary: Path, home: Path, codex_home: Path) -> None:
    import fcntl
    import pty
    import struct
    import termios

    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
    environment = os.environ.copy()
    environment.update({"TERM": "xterm-256color", "CODEX_HOME": str(codex_home)})
    process = subprocess.Popen(
        [str(binary), "--home", str(home), "--no-color"],
        stdin=slave,
        stdout=slave,
        stderr=slave,
        env=environment,
        start_new_session=True,
    )
    os.close(slave)

    def read_chunk() -> str:
        ready, _, _ = select.select([master], [], [], 0.25)
        if not ready:
            return ""
        return os.read(master, 65536).decode("utf-8", errors="replace")

    try:
        wait_for(read_chunk, "▶ Today")
        os.write(master, b"\x1b[C")
        wait_for(read_chunk, "▶ Week")
        os.write(master, b"/")
        wait_for(read_chunk, "Commands")
        os.write(master, b"q")
        wait_for(read_chunk, "/q▌")
        if process.poll() is not None:
            raise AssertionError("q exited while the slash palette was open")
        os.write(master, b"\x1b")
        wait_for(read_chunk, "Scope · All projects")
        os.write(master, b"q")
        process.wait(timeout=8)
        if process.returncode != 0:
            raise AssertionError(f"interactive process exited with {process.returncode}")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)


def run_windows(binary: Path, home: Path, codex_home: Path) -> None:
    from winpty import PtyProcess

    environment = os.environ.copy()
    environment.update({"TERM": "xterm-256color", "CODEX_HOME": str(codex_home)})
    process = PtyProcess.spawn(
        [str(binary), "--home", str(home), "--no-color"],
        cwd=str(binary.parent),
        env=environment,
        dimensions=(30, 120),
    )
    process.fileobj.settimeout(0.25)

    def read_chunk() -> str:
        return process.read(65536)

    try:
        wait_for(read_chunk, "▶ Today")
        process.write("\x1b[C")
        wait_for(read_chunk, "▶ Week")
        process.write("/")
        wait_for(read_chunk, "Commands")
        process.write("q")
        wait_for(read_chunk, "/q▌")
        if not process.isalive():
            raise AssertionError("q exited while the slash palette was open")
        process.write("\x1b")
        wait_for(read_chunk, "Scope · All projects")
        process.write("q")
        deadline = time.monotonic() + 8
        while process.isalive() and time.monotonic() < deadline:
            time.sleep(0.05)
        if process.isalive():
            raise AssertionError("interactive process did not exit after Esc then q")
        if process.exitstatus != 0:
            raise AssertionError(f"interactive process exited with {process.exitstatus}")
    finally:
        if process.isalive():
            process.terminate(force=True)
        process.close(force=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file():
        raise SystemExit(f"binary not found: {binary}")
    with tempfile.TemporaryDirectory(prefix="codex-meter-pty-") as temporary:
        root = Path(temporary)
        if os.name == "nt":
            run_windows(binary, root / "meter", root / "codex")
        else:
            run_posix(binary, root / "meter", root / "codex")
    print(f"native PTY interaction passed on {sys.platform}: arrows, slash-q, Esc-q")


if __name__ == "__main__":
    main()
