#!/usr/bin/env node
// Validate the updater manifest generated for a tagged release before it is
// published or copied to the preview feed.
import { readFileSync } from "node:fs";

const [manifestPath, tag] = process.argv.slice(2);
if (!manifestPath || !tag?.startsWith("v")) {
  console.error("usage: verify-updater-manifest.mjs latest.json vX.Y.Z[-prerelease]");
  process.exit(2);
}

const expectedVersion = tag.slice(1);
const requiredPlatforms = [
  "darwin-aarch64",
  "darwin-x86_64",
  "linux-x86_64",
  "linux-x86_64-deb",
];

let manifest;
try {
  manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
} catch (error) {
  console.error(`cannot parse ${manifestPath}: ${error.message}`);
  process.exit(1);
}

if (manifest.version !== expectedVersion) {
  console.error(`manifest version ${manifest.version} != ${expectedVersion}`);
  process.exit(1);
}

let failed = false;
for (const platform of requiredPlatforms) {
  const entry = manifest.platforms?.[platform];
  if (!entry) {
    console.error(`missing updater platform: ${platform}`);
    failed = true;
    continue;
  }
  if (typeof entry.url !== "string" || !entry.url.includes(`/releases/download/${tag}/`)) {
    console.error(`${platform} URL is not pinned to ${tag}: ${entry.url}`);
    failed = true;
  }
  if (typeof entry.signature !== "string" || entry.signature.trim().length === 0) {
    console.error(`${platform} has an empty updater signature`);
    failed = true;
  } else {
    console.log(`updater platform ok: ${platform}`);
  }
}

if (failed) process.exit(1);
console.log(`updater manifest ok for ${tag}.`);
