import { describe, expect, it } from "vitest";
import {
  containerStateLabel,
  effectiveProgress,
  formatBytes,
  isModelField,
  managedFilePath,
  modelOptionsForField,
  modelStateLabel,
  percentOf,
  phaseLabel,
  purposeLabel,
  weightsStateLabel,
  type ModelCatalog,
  type ModelEntry,
} from "../src/lib/models";

function entry(overrides: Partial<ModelEntry> = {}): ModelEntry {
  return {
    id: "kokoro-v1.0",
    label: "Kokoro v1.0 (ONNX TTS)",
    purpose: "tts",
    backend: "kokoro",
    state: "installed",
    active: false,
    total_bytes: 353_746_785,
    requires: [],
    files: [
      {
        path: "kokoro-v1.0.onnx",
        url: "https://example.invalid/kokoro-v1.0.onnx",
        size_bytes: 325_532_387,
        sha256: null,
        present: true,
        verified: true,
        legacy: false,
      },
      {
        path: "voices-v1.0.bin",
        url: "https://example.invalid/voices-v1.0.bin",
        size_bytes: 28_214_398,
        sha256: null,
        present: true,
        verified: true,
        legacy: false,
      },
    ],
    download: null,
    ...overrides,
  };
}

function catalog(models: ModelEntry[], root = "/home/tester/.local/share/seneschal/models"): ModelCatalog {
  return { root, models };
}

describe("model manager helpers", () => {
  it("formats byte sizes for the catalog cards", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(939)).toBe("939 B");
    expect(formatBytes(885_098)).toBe("864 KB");
    expect(formatBytes(325_532_387)).toBe("310 MB");
    expect(formatBytes(1_624_555_275)).toBe("1.5 GB");
    expect(formatBytes(Number.NaN)).toBe("—");
  });

  it("computes bounded download percentages", () => {
    expect(percentOf(0, 100)).toBe(0);
    expect(percentOf(50, 100)).toBe(50);
    expect(percentOf(150, 100)).toBe(100);
    expect(percentOf(10, 0)).toBe(0);
    expect(percentOf(10, null)).toBe(0);
  });

  it("maps wire tokens to readable labels", () => {
    expect(modelStateLabel("partial")).toBe("Partial");
    expect(phaseLabel("cancelled")).toBe("Cancelled");
    expect(purposeLabel("stt")).toBe("STT");
    expect(containerStateLabel("model_mismatch")).toBe("Model mismatch");
    expect(weightsStateLabel("downloading")).toBe("Downloading");
  });

  it("resolves managed absolute paths from the catalog root", () => {
    const entryKokoro = entry();
    expect(managedFilePath(catalog([entryKokoro]), entryKokoro, (p) => p.endsWith(".onnx"))).toBe(
      "/home/tester/.local/share/seneschal/models/kokoro-v1.0.onnx",
    );
    expect(managedFilePath(catalog([entryKokoro]), entryKokoro, (p) => p.endsWith(".missing"))).toBeNull();
    expect(managedFilePath(null, entryKokoro)).toBeNull();
  });

  it("builds Settings selector options from installed models only", () => {
    const entries = [
      entry(),
      entry({
        id: "whisper-large-v3-turbo",
        label: "Whisper large-v3-turbo",
        backend: "whisper",
        purpose: "stt",
        files: [
          {
            path: "ggml-large-v3-turbo.bin",
            url: "https://example.invalid/ggml-large-v3-turbo.bin",
            size_bytes: 1_624_555_275,
            sha256: null,
            present: true,
            verified: true,
            legacy: false,
          },
        ],
      }),
      entry({
        id: "whisper-tiny",
        label: "Whisper tiny",
        backend: "whisper",
        purpose: "stt",
        state: "missing",
        files: [
          {
            path: "ggml-tiny.bin",
            url: "https://example.invalid/ggml-tiny.bin",
            size_bytes: 1,
            sha256: null,
            present: false,
            verified: false,
            legacy: false,
          },
        ],
      }),
    ];
    const cat = catalog(entries);

    const whisper = modelOptionsForField(cat, "whisper_model");
    expect(whisper.map((option) => option.modelId)).toEqual(["whisper-large-v3-turbo"]);
    expect(whisper[0].value).toContain("ggml-large-v3-turbo.bin");

    const kokoro = modelOptionsForField(cat, "kokoro_model");
    expect(kokoro).toHaveLength(1);
    expect(kokoro[0].value).toContain("kokoro-v1.0.onnx");
    expect(kokoro[0].voices).toContain("voices-v1.0.bin");

    expect(modelOptionsForField(cat, "vad_model")).toEqual([]);
    expect(modelOptionsForField(null, "whisper_model")).toEqual([]);
  });

  it("maps parakeet bundles to their directory", () => {
    const parakeet = entry({
      id: "parakeet-tdt-0.6b-v3",
      backend: "parakeet",
      purpose: "stt",
      files: [
        {
          path: "parakeet-tdt-0.6b-v3-onnx/encoder-model.onnx",
          url: "https://example.invalid/encoder-model.onnx",
          size_bytes: 1,
          sha256: null,
          present: true,
          verified: true,
          legacy: false,
        },
      ],
    });
    const options = modelOptionsForField(catalog([parakeet]), "parakeet_model_dir");
    expect(options).toHaveLength(1);
    expect(options[0].value).toBe(
      "/home/tester/.local/share/seneschal/models/parakeet-tdt-0.6b-v3-onnx",
    );
  });

  it("classifies the Settings fields backed by the catalog", () => {
    expect(isModelField("kokoro_model")).toBe(true);
    expect(isModelField("vad_model")).toBe(true);
    expect(isModelField("stt_model")).toBe(false);
    expect(isModelField("llm_model")).toBe(false);
  });

  it("prefers live SSE progress over the catalog snapshot", () => {
    const model = entry({
      state: "downloading",
      download: {
        download_id: "d1",
        phase: "downloading",
        downloaded_bytes: 10,
        total_bytes: 100,
        percent: 10,
        current_file: null,
        message: null,
      },
    });

    const live = effectiveProgress(model, {
      "kokoro-v1.0": {
        modelId: "kokoro-v1.0",
        downloadId: "d2",
        phase: "downloading",
        downloadedBytes: 60,
        totalBytes: 100,
        currentFile: "kokoro-v1.0.onnx",
        message: null,
      },
    });
    expect(live?.downloaded).toBe(60);
    expect(live?.total).toBe(100);

    const snapshot = effectiveProgress(model, {});
    expect(snapshot?.downloaded).toBe(10);

    const idle = entry({ state: "installed", download: null });
    expect(effectiveProgress(idle, {})).toBeNull();
  });
});
