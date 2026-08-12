# Build from source

The supported development platform for the first public release is Linux. Python 3.11 or newer, Git, and the standard `venv` module are required.

## Clone and install in a virtual environment

```bash
git clone https://github.com/DelicateNorman/codex-meter.git
cd codex-meter
python3 -m venv .venv
source .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e .
codex-meter --version
codex-meter
```

The editable installation runs directly from the cloned source tree. Deactivate it with `deactivate`.

## Run without installing

From the repository root:

```bash
python3 -m codex_meter --version
python3 -m codex_meter
```

## Test

```bash
python3 -m unittest discover -v
```

## Build a wheel

```bash
python3 -m pip install build
python3 -m build
python3 -m pip install dist/codex_meter-*.whl
```

Build outputs are written to `dist/` and are intentionally excluded from Git.

## Platform status

- Linux: supported and tested.
- macOS: planned; source may run, but it is not yet a supported release target.
- Windows: planned; terminal input and OS-specific paths still need an implementation.
