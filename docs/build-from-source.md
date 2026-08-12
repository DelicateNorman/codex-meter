# Build from source

Codex Meter 0.16 and later are built from Rust. The Python 0.15 implementation
remains in the repository as a compatibility reference and regression oracle;
official 0.16 release assets use the Rust executable.

## Current Rust version

Install [Rust with rustup](https://rustup.rs/), then run:

```bash
git clone https://github.com/DelicateNorman/codex-meter.git
cd codex-meter
cargo test --all-targets --locked
cargo build --release --locked
./target/release/codex-meter --no-color demo
./target/release/codex-meter
```

On Windows PowerShell, the executable is
`target\release\codex-meter.exe`. The Rust executable does not need Python at
runtime. It deliberately reuses `~/.codex-meter/config.toml`, `pricing.json`,
and `meter.db`, so existing history is preserved.

## Legacy Python v0.15 reference

Use this section only when reproducing the old v0.15 implementation or running
the differential test suite. It supports Python 3.11 or newer and Git and has no
third-party runtime dependencies.

## Linux and macOS

```bash
git clone https://github.com/DelicateNorman/codex-meter.git
cd codex-meter
git switch --detach v0.15.0
python3 -m venv .venv
source .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e .
python -m unittest discover -v
codex-meter
```

## Windows PowerShell

```powershell
git clone https://github.com/DelicateNorman/codex-meter.git
Set-Location codex-meter
git switch --detach v0.15.0
py -3 -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install --upgrade pip
python -m pip install -e .
python -m unittest discover -v
codex-meter
```

If PowerShell blocks the activation script, run `Set-ExecutionPolicy -Scope Process Bypass` for that terminal only, then activate it again.

The editable installation runs directly from the cloned source tree. Leave it with `deactivate`.

## Run legacy Python without installing

From the repository root:

```bash
python3 -m codex_meter --version
python3 -m codex_meter
```

On Windows, use `py -3` in place of `python3` when needed.

## Build a legacy Python wheel

```bash
python -m pip install build
python -m build
python -m pip install dist/codex_meter-*.whl
```

Build outputs are written to `dist/` and are intentionally excluded from Git.

## Build a legacy Python standalone executable

PyInstaller must run on the target operating system; it does not cross-compile. From an activated environment:

```bash
python -m pip install "pyinstaller>=6.11,<7"
pyinstaller --noconfirm --clean --onefile --name codex-meter \
  --collect-data codex_meter --collect-submodules email \
  --hidden-import email.quoprimime --hidden-import email.base64mime \
  packaging/codex_meter_entry.py
```

The result is `dist/codex-meter` on Linux/macOS or `dist/codex-meter.exe` on
Windows. Current tagged GitHub releases instead build and test the Rust
executables automatically on Linux x86_64, macOS arm64/x86_64, and Windows
x86_64.

## Platform notes

- Codex state is read from `$CODEX_HOME`, with the official default `~/.codex` on every platform.
- Codex Meter state defaults to `~/.codex-meter` and can be changed with `CODEX_METER_HOME` or `--home`.
- Native Windows uses Win32 console input; Linux, macOS, and WSL use POSIX terminal input.
- If Codex runs inside WSL2, build or install Codex Meter inside the same distribution.
- Passive tcpdump metadata capture is automatic on Linux/macOS. It is optional and normally unavailable on native Windows; the Network response and socket-probe views still work.
- Explicit TLS diagnostics require an `openssl` executable on `PATH`; all other features work without it.
