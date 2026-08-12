# Codex Meter for macOS

Codex Meter Desktop is a native companion app for people who prefer a normal
window over a terminal dashboard. It does not replace Codex CLI and does not
change Codex Desktop. It reads the same metadata already used by the
`codex-meter` CLI.

## What it shows

- current Codex and Spark seven-day account limits;
- token usage, API-equivalent cost, cache efficiency, calls, and sessions;
- Today, current Week, current Month, and All time views;
- per-project filtering, with recently used projects first;
- response timing, model and reasoning-effort breakdowns;
- recent sessions and incremental local/SSH refresh progress.

The first refresh imports existing Rollout metadata. Later refreshes skip
unchanged files. A failed remote server is reported without hiding successfully
refreshed local data.

## Data and privacy

The app and CLI share `~/.codex-meter/meter.db`. Installing, opening, updating,
or removing the desktop app does not delete this directory. Existing CLI
history therefore appears automatically in the desktop app.

Codex Meter retains usage and timing metadata only. It does not persist prompts,
responses, reasoning text, shell commands, tool arguments or output, HTTP
headers, cookies, credentials, or authentication files. Remote sources use the
same metadata-only SSH filter as the CLI.

## Use the app

1. Open **Codex Meter** from Applications.
2. Choose Today, Week, Month, or All time at the top of Overview.
3. Leave Project on **All projects**, or select one project.
4. Click the circular arrow to import changed local and remote metadata.
5. Open Sessions for recent work, or Settings to inspect data paths and manage
   SSH aliases.

Weekly account limits load in parallel and are independent of the selected
date or project. If they are unavailable, confirm `codex --version` works in a
terminal and refresh. The app searches the normal shell PATH plus common macOS
Homebrew, local-bin, Volta, FNM, npm-global, and NVM locations.

To add a server, first make sure `ssh alias` works from Terminal. Then open
Settings, enter the same alias, and choose **Add server**. The first metadata
sync can take longer; progress remains visible and later syncs are incremental.

## Platform and preview builds

The current target is macOS 12 or newer on Apple Silicon and Intel. The
[macOS desktop workflow](https://github.com/DelicateNorman/codex-meter/actions/workflows/desktop.yml)
produces both architectures and validates each app bundle and DMG. These CI
artifacts are unsigned development previews. A public installer should be
Developer ID signed and notarized before general distribution.

## Build from source

Install the current Node.js LTS release and stable Rust, then run:

```bash
git clone https://github.com/DelicateNorman/codex-meter.git
cd codex-meter/desktop
npm ci
npm run tauri dev
```

To create a release app and DMG:

```bash
npm run tauri build
```

Artifacts are written under `desktop/src-tauri/target/release/bundle/`. Building
does not require changing or moving `~/.codex-meter`.
