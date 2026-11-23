import { describe, expect, it } from "vitest";
import {
  originLabel,
  settingValue,
  tierLabel,
  type SettingsField,
} from "../src/lib/settings";

describe("settings presentation helpers", () => {
  it("labels effective-value origins", () => {
    expect(originLabel("default")).toBe("Default");
    expect(originLabel("file")).toBe("File");
    expect(originLabel("environment")).toBe("Env");
  });

  it("labels apply tiers in user language", () => {
    expect(tierLabel("hot")).toBe("No restart");
    expect(tierLabel("restart")).toBe("Restart");
  });

  it("renders optional values as empty text", () => {
    expect(settingValue(null)).toBe("");
    expect(settingValue(undefined)).toBe("");
    expect(settingValue(false)).toBe("false");
    expect(settingValue(0.5)).toBe("0.5");
  });

  it("keeps provider metadata in field shape", () => {
    const field: SettingsField = {
      key: "tts_provider",
      label: "TTS backend",
      category: "tts",
      value: "kokoro",
      origin: "file",
      tier: "restart",
      editable: true,
      options: ["kokoro", "openai"],
    };
    expect(field.options).toEqual(["kokoro", "openai"]);
    expect(field.editable).toBe(true);
  });
});
