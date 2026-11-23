import { writable } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type DaemonStatus =
  | "starting"
  | "running"
  | "restarting"
  | "stopped"
  | "failed";

export type ConnectionStatus = "disconnected" | "connecting" | "connected";

export type PipelineToken =
  | "idle"
  | "listening"
  | "thinking"
  | "speaking"
  | "paused";

export interface PipelineSnapshot {
  state: PipelineToken | string;
  utteranceId: number | null;
  pauseReason: string | null;
  ttsMuted: boolean | null;
}

export interface DesktopStatus {
  daemon: DaemonStatus;
  connection: ConnectionStatus;
  pipeline: PipelineSnapshot | null;
  lastError: string | null;
  lastReconnectMs: number | null;
}

export const status = writable<DesktopStatus | null>(null);

let unlisten: UnlistenFn | null = null;

export async function initStatus(): Promise<void> {
  if (unlisten) return;
  // Subscribe first so a status change during startup cannot be lost between
  // the initial snapshot and the event listener.
  unlisten = await listen<DesktopStatus>("desktop-status", (event) => {
    status.set(event.payload);
  });
  const initial = await invoke<DesktopStatus>("desktop_status");
  status.set(initial);
}

export async function restartDaemon(): Promise<void> {
  await invoke("restart_daemon");
}

export function daemonLabel(value: DaemonStatus): string {
  const labels: Record<DaemonStatus, string> = {
    starting: "Starting",
    running: "Running",
    restarting: "Restarting",
    stopped: "Stopped",
    failed: "Failed",
  };
  return labels[value] ?? value;
}

export function connectionLabel(value: ConnectionStatus): string {
  const labels: Record<ConnectionStatus, string> = {
    disconnected: "Disconnected",
    connecting: "Connecting",
    connected: "SSE connected",
  };
  return labels[value] ?? value;
}

export function pipelineLabel(value: string): string {
  const labels: Record<PipelineToken, string> = {
    idle: "Idle",
    listening: "Listening",
    thinking: "Thinking",
    speaking: "Speaking",
    paused: "Paused",
  };
  return labels[value as PipelineToken] ?? value;
}

export function formatReconnect(value: number | null | undefined): string {
  return value == null ? "—" : `${value} ms`;
}
