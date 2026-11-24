import { Channel, invoke } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";

export interface UpdateInfo {
  currentVersion: string;
  version: string;
  notes: string | null;
  publishedAt: string | null;
}

export type UpdateProgressEvent =
  | { event: "Started"; data: { contentLength: number | null } }
  | { event: "Progress"; data: { chunkLength: number } }
  | { event: "Finished" };

export interface UpdateProgressState {
  downloaded: number | null;
  total: number | null;
  finished: boolean;
}

export function shouldCheckUpdates(mode: string): boolean {
  // `tauri dev` has no signed production artifact to compare against.
  return mode !== "development";
}

export async function checkForUpdate(): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>("check_for_update");
}

export async function installUpdate(
  onEvent: (event: UpdateProgressEvent) => void,
): Promise<void> {
  const channel = new Channel<UpdateProgressEvent>();
  channel.onmessage = onEvent;
  await invoke("install_update", { onEvent: channel });
}

export async function relaunchAfterUpdate(): Promise<void> {
  await relaunch();
}

/** Fold raw updater events into the small state rendered by the shell. */
export function nextUpdateProgress(
  previous: UpdateProgressState | null,
  event: UpdateProgressEvent,
): UpdateProgressState {
  const base: UpdateProgressState = previous ?? {
    downloaded: 0,
    total: null,
    finished: false,
  };
  switch (event.event) {
    case "Started":
      return { ...base, downloaded: 0, total: event.data.contentLength, finished: false };
    case "Progress":
      return {
        ...base,
        downloaded: (base.downloaded ?? 0) + event.data.chunkLength,
        finished: false,
      };
    case "Finished":
      return { ...base, finished: true };
  }
}

export function updateProgressLabel(progress: UpdateProgressState): string {
  if (progress.finished) return "Installed — restarting…";
  if (progress.total && progress.total > 0 && progress.downloaded !== null) {
    const percent = Math.min(100, Math.round((progress.downloaded / progress.total) * 100));
    return `Downloading ${percent}%`;
  }
  return "Downloading…";
}
