"""PyInstaller entry point for standalone Codex Meter releases."""

# PyInstaller's stdlib email hook can omit these lazy imports on some Python
# distributions. Importing them explicitly keeps bundled HTTP/OTLP support
# identical to source installs.
import email.base64mime  # noqa: F401
import email.quoprimime  # noqa: F401

from codex_meter.cli import main


raise SystemExit(main())
