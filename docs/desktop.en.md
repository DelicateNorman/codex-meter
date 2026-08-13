# Codex Meter for macOS

Codex Meter Desktop is a native companion app for people who prefer a normal
window over a terminal dashboard. It does not replace Codex CLI and does not
change Codex Desktop. It reads the same metadata already used by the
`codex-meter` CLI.

## What it shows

- current Codex and Spark seven-day account limits;
- token usage, API-equivalent cost, cache efficiency, calls, and sessions;
- dated Day, Week, Month, and All-time views, plus calendar history;
- project and optional manual account filters;
- cache savings, retries, response speed, and metadata-only network insights;
- CSV export, recent sessions, and incremental local/SSH refresh progress;
- per-server online tests, status, errors, refresh, and safe cancellation.

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
5. Open History, Insights, or Sessions for detail; Settings manages account
   labels, the pricing catalog, data paths, and SSH aliases.

Use `⌘R` to refresh, `⌘,` for Settings, and `⌘1`/`⌘2`/`⌘3` to switch among
Overview, History, and Insights. Date arrows move the selected report window;
Export writes only filtered statistical metadata to CSV.

Weekly account limits load in parallel and are independent of the selected
date or project. If they are unavailable, confirm `codex --version` works in a
terminal and refresh. The app searches the normal shell PATH plus common macOS
Homebrew, local-bin, Volta, FNM, npm-global, and NVM locations.

To add a server, first make sure `ssh alias` works from Terminal. Then open
Settings, enter the same alias, and choose **Add server**. The first metadata
sync can take longer; progress remains visible and later syncs are incremental.

## Install the public preview

The current target is macOS 12 or newer. Download the DMG for your Mac:

- [Apple Silicon (M1/M2/M3/M4)](https://github.com/DelicateNorman/codex-meter/releases/download/v0.17.0-beta.1/codex-meter-desktop-macos-arm64.dmg)
- [Intel](https://github.com/DelicateNorman/codex-meter/releases/download/v0.17.0-beta.1/codex-meter-desktop-macos-x86_64.dmg)

Open the DMG and drag Codex Meter into Applications. These preview builds are
unsigned and not notarized, so macOS may block the first launch. Right-click
Codex Meter and choose **Open**. If it is still blocked, open **System Settings
→ Privacy & Security** and choose **Open Anyway** for Codex Meter. Only use a
download whose SHA-256 value matches `SHA256SUMS` on the Release page.

No paid Apple developer membership is required to build or test the app. A
future generally available build should use Developer ID signing and Apple
notarization to remove this extra first-launch step.

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
