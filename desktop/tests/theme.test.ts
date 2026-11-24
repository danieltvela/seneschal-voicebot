import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import tauriConf from "../src-tauri/tauri.conf.json";

type WindowConfig = { label: string; backgroundColor?: string };

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const appCss = readFileSync(resolve(root, "src/app.css"), "utf8");
const mainTs = readFileSync(resolve(root, "src/main.ts"), "utf8");

describe("desktop document reset (issue #39)", () => {
  it("resets html and body to the app background", () => {
    expect(appCss).toMatch(/html,\s*\n\s*body\s*\{/);
    expect(appCss).toMatch(/\bmargin:\s*0\b/);
    expect(appCss).toMatch(/\bbackground:\s*#101317\b/);
  });

  it("is imported by the frontend entrypoint", () => {
    expect(mainTs).toContain('import "./app.css";');
  });

  it("paints the window background before first paint", () => {
    const main = (tauriConf.app.windows as WindowConfig[]).find(
      (w) => w.label === "main",
    );
    expect(main).toBeDefined();
    expect(main?.backgroundColor).toBe("#101317");
  });
});
