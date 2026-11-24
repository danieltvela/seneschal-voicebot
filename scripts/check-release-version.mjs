#!/usr/bin/env node
// Release guard: the shipped desktop version must equal the version encoded in
// the Git tag. This keeps updater version comparison and release assets aligned.
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tag = process.argv[2];

if (!tag || !tag.startsWith("v")) {
  console.error("usage: check-release-version.mjs vX.Y.Z[-prerelease]");
  process.exit(2);
}
const expected = tag.slice(1);

const readJson = (path) => JSON.parse(readFileSync(resolve(root, path), "utf8"));
const cargoVersion = (path) => {
  const source = readFileSync(resolve(root, path), "utf8");
  const start = source.indexOf("[package]");
  if (start < 0) throw new Error(`missing [package] section in ${path}`);
  const nextSection = source.indexOf("\n[", start + 1);
  const section = source.slice(start, nextSection < 0 ? undefined : nextSection + 1);
  const version = section.match(/^version\s*=\s*"([^"]+)"\s*$/m);
  if (!version) throw new Error(`missing package version in ${path}`);
  return version[1];
};

const manifestChecks = [
  ["Cargo.toml", cargoVersion("Cargo.toml")],
  ["desktop/src-tauri/Cargo.toml", cargoVersion("desktop/src-tauri/Cargo.toml")],
  ["desktop/package.json", readJson("desktop/package.json").version],
  [
    "desktop/src-tauri/tauri.conf.json",
    readJson("desktop/src-tauri/tauri.conf.json").version,
  ],
];

const packageLock = readJson("desktop/package-lock.json");
manifestChecks.push([
  "desktop/package-lock.json",
  packageLock.packages?.[""]?.version,
]);

let failed = false;
for (const [path, actual] of manifestChecks) {
  if (actual === expected) {
    console.log(`version ok: ${path} = ${actual}`);
    continue;
  }
  console.error(`version mismatch: ${path} = ${actual}, expected ${expected}`);
  failed = true;
}

if (failed) process.exit(1);
