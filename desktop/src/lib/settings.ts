import { writable } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";

export type SettingOrigin = "default" | "file" | "environment";
export type SettingTier = "hot" | "restart";
export type SettingCategory = "llm" | "stt" | "tts" | "general" | "audio" | "network";

export interface SettingsField {
  key: string;
  label: string;
  category: SettingCategory | string;
  value: unknown;
  origin: SettingOrigin;
  tier: SettingTier;
  editable: boolean;
  options?: string[];
}

export interface SettingsProviders {
  stt: string[];
  tts: string[];
}

export interface SettingsSnapshot {
  revision: number;
  config_file: string | null;
  fields: SettingsField[];
  providers: SettingsProviders;
}

export interface FieldError {
  field: string;
  message: string;
}

export interface ValidationResponse {
  ok: boolean;
  errors: FieldError[];
  tier: SettingTier | null;
}

export interface SettingsApplyResponse {
  tier: SettingTier;
  revision: number;
  snapshot: SettingsSnapshot;
}

export type SettingsPatch = Record<string, unknown>;

export const settings = writable<SettingsSnapshot | null>(null);

export async function loadSettings(): Promise<void> {
  const snapshot = await invoke<SettingsSnapshot>("get_settings");
  settings.set(snapshot);
}

export async function validateSettings(
  patch: SettingsPatch,
): Promise<ValidationResponse> {
  return invoke<ValidationResponse>("validate_settings", { patch });
}

export async function applySettings(
  patch: SettingsPatch,
): Promise<SettingsApplyResponse> {
  return invoke<SettingsApplyResponse>("apply_settings", { patch });
}

export function originLabel(origin: SettingOrigin): string {
  const labels: Record<SettingOrigin, string> = {
    default: "Default",
    file: "File",
    environment: "Env",
  };
  return labels[origin] ?? origin;
}

export function tierLabel(tier: SettingTier): string {
  return tier === "hot" ? "No restart" : "Restart";
}

export function settingValue(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "boolean") return value ? "true" : "false";
  return String(value);
}
