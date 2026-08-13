#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const write = process.argv.includes("--write");
if (!write && !process.argv.includes("--check")) {
  console.error("Usage: node scripts/sync-versions.mjs --check|--write");
  process.exit(2);
}

const read = relative => fs.readFileSync(path.join(root, relative), "utf8");
const cargo = read("Cargo.toml");
const version = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) throw new Error("Could not read the root Cargo package version");

const expected = new Map();

for (const relative of ["desktop/package.json", "desktop/package-lock.json"]) {
  const document = JSON.parse(read(relative));
  document.version = version;
  if (document.packages?.[""]) document.packages[""].version = version;
  expected.set(relative, `${JSON.stringify(document, null, 2)}\n`);
}

const tauriConfig = JSON.parse(read("desktop/src-tauri/tauri.conf.json"));
tauriConfig.version = "../package.json";
expected.set("desktop/src-tauri/tauri.conf.json", `${JSON.stringify(tauriConfig, null, 2)}\n`);

const desktopCargo = read("desktop/src-tauri/Cargo.toml").replace(
  /(^\[package\][\s\S]*?^version\s*=\s*)"[^"]+"/m,
  `$1"${version}"`,
);
expected.set("desktop/src-tauri/Cargo.toml", desktopCargo);

const installer = read("install.sh").replace(
  /^version="\$\{CODEX_METER_VERSION:-v[^}]+\}"/m,
  `version="\${CODEX_METER_VERSION:-v${version}}"`,
);
expected.set("install.sh", installer);

const powershellInstaller = read("install.ps1").replace(
  /\{ "v[^"}]+" \}\),/,
  `{ "v${version}" }),`,
);
expected.set("install.ps1", powershellInstaller);

const mismatches = [];
for (const [relative, content] of expected) {
  if (read(relative) === content) continue;
  mismatches.push(relative);
  if (write) fs.writeFileSync(path.join(root, relative), content);
}

if (mismatches.length && !write) {
  console.error(`Version ${version} is not synchronized in: ${mismatches.join(", ")}`);
  console.error("Run: node scripts/sync-versions.mjs --write");
  process.exit(1);
}

console.log(`${write ? "Synchronized" : "Verified"} version ${version}${mismatches.length ? ` in ${mismatches.length} file(s)` : ""}.`);
