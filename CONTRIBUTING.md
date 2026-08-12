# Contributing

Codex Meter currently targets Linux and Python 3.11+. Bug reports, privacy reviews, documentation fixes, and focused pull requests are welcome.

## Development setup

```bash
git clone https://github.com/DelicateNorman/codex-meter.git
cd codex-meter
python3 -m venv .venv
source .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e .
python -m unittest discover -v
```

Please keep collection content-free: prompts, responses, reasoning text, commands, tool output, headers, cookies, credentials, and authentication files must never be persisted.

Before opening a pull request, run the full test suite and describe any user-visible behavior changes. Platform work for macOS and Windows should include platform-specific installation and terminal-navigation tests.
