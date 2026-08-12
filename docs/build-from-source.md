# Build from source

Codex Meter supports Linux, macOS, and native Windows with Python 3.11 or newer and Git. The runtime itself has no third-party Python dependencies.

## Linux and macOS

```bash
git clone https://github.com/DelicateNorman/codex-meter.git
cd codex-meter
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
py -3 -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install --upgrade pip
python -m pip install -e .
python -m unittest discover -v
codex-meter
```

If PowerShell blocks the activation script, run `Set-ExecutionPolicy -Scope Process Bypass` for that terminal only, then activate it again.

The editable installation runs directly from the cloned source tree. Leave it with `deactivate`.

## Run without installing

From the repository root:

```bash
python3 -m codex_meter --version
python3 -m codex_meter
```

On Windows, use `py -3` in place of `python3` when needed.

## Build a wheel

```bash
python -m pip install build
python -m build
python -m pip install dist/codex_meter-*.whl
```

Build outputs are written to `dist/` and are intentionally excluded from Git.

## Build a standalone executable

PyInstaller must run on the target operating system; it does not cross-compile. From an activated environment:

```bash
python -m pip install "pyinstaller>=6.11,<7"
pyinstaller --noconfirm --clean --onefile --name codex-meter \
  --collect-data codex_meter --collect-submodules email \
  --hidden-import email.quoprimime --hidden-import email.base64mime \
  packaging/codex_meter_entry.py
```

The result is `dist/codex-meter` on Linux/macOS or `dist/codex-meter.exe` on Windows. Tagged GitHub releases build and smoke-test Linux x86_64, macOS arm64/x86_64, and Windows x86_64 executables automatically.

## Platform notes

- Codex state is read from `$CODEX_HOME`, with the official default `~/.codex` on every platform.
- Codex Meter state defaults to `~/.codex-meter` and can be changed with `CODEX_METER_HOME` or `--home`.
- Native Windows uses Win32 console input; Linux, macOS, and WSL use POSIX terminal input.
- If Codex runs inside WSL2, build or install Codex Meter inside the same distribution.
- Passive tcpdump metadata capture is automatic on Linux/macOS. It is optional and normally unavailable on native Windows; the Network response and socket-probe views still work.
- Explicit TLS diagnostics require an `openssl` executable on `PATH`; all other features work without it.
