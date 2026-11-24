import { describe, expect, it } from "vitest";
import {
  nextUpdateProgress,
  shouldCheckUpdates,
  updateProgressLabel,
  type UpdateProgressEvent,
} from "../src/lib/update";

describe("desktop updater helpers", () => {
  it("skips update checks in development only", () => {
    expect(shouldCheckUpdates("development")).toBe(false);
    expect(shouldCheckUpdates("production")).toBe(true);
    expect(shouldCheckUpdates("test")).toBe(true);
  });

  it("folds started, progress and finish events", () => {
    const started = nextUpdateProgress(null, {
      event: "Started",
      data: { contentLength: 100 },
    } satisfies UpdateProgressEvent);
    expect(started).toEqual({ downloaded: 0, total: 100, finished: false });

    const progressing = nextUpdateProgress(started, {
      event: "Progress",
      data: { chunkLength: 40 },
    } satisfies UpdateProgressEvent);
    expect(progressing).toEqual({ downloaded: 40, total: 100, finished: false });

    const finished = nextUpdateProgress(progressing, {
      event: "Finished",
    } satisfies UpdateProgressEvent);
    expect(finished).toEqual({ downloaded: 40, total: 100, finished: true });
    expect(updateProgressLabel(finished)).toContain("restarting");
  });

  it("handles unknown content length and percentage bounds", () => {
    const unknown = nextUpdateProgress(null, {
      event: "Started",
      data: { contentLength: null },
    } satisfies UpdateProgressEvent);
    expect(updateProgressLabel(unknown)).toBe("Downloading…");

    const oversized = nextUpdateProgress(
      nextUpdateProgress(null, {
        event: "Started",
        data: { contentLength: 1 },
      } satisfies UpdateProgressEvent),
      { event: "Progress", data: { chunkLength: 10_000 } } satisfies UpdateProgressEvent,
    );
    expect(updateProgressLabel(oversized)).toBe("Downloading 100%");
  });
});
