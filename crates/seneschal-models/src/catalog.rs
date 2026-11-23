//! Static catalog of downloadable models (issue #35).
//!
//! Sourced from the `install.sh` registry (whisper, Silero VAD, Parakeet,
//! Kokoro) plus the docker weight sets for the NeMo services. Sizes and
//! SHA-256 digests were captured from the upstream artifacts (HF LFS pointers
//! for Hugging Face files; direct hashes for the GitHub release assets).
//!
//! NeMo GGUF weight sets are not listed here: they are fetched through
//! `nemo-speech pull` (which verifies size + SHA-256 itself) and are surfaced
//! as `seneschal_models::nemo` service cards instead.

use serde::Serialize;

/// What a model is used for in the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPurpose {
    Stt,
    Tts,
    Vad,
    Speaker,
}

/// Which backend consumes the model. Drives activation (which settings keys
/// the model manager writes when the user picks "use this model").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelBackend {
    Whisper,
    Parakeet,
    Silero,
    Kokoro,
    NemotronAsr,
    MagpieTts,
}

/// One downloadable artifact of a model bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFile {
    /// Path relative to the models root.
    pub rel_path: String,
    pub url: String,
    pub size_bytes: u64,
    /// SHA-256 (lowercase hex) of the file contents. `None` is only valid for
    /// artifacts whose integrity is verified by an upstream tool.
    pub sha256: Option<String>,
}

/// A selectable model: one or more files plus its dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    pub id: String,
    pub label: String,
    pub purpose: ModelPurpose,
    pub backend: ModelBackend,
    pub files: Vec<ModelFile>,
    /// Model ids that must be installed before this model can be activated.
    pub requires: Vec<String>,
}

impl ModelSpec {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|file| file.size_bytes).sum()
    }

    /// Common parent directory of all files, when they share one (Parakeet).
    pub fn bundle_dir(&self) -> Option<String> {
        let first = self.files.first()?;
        let parent = std::path::Path::new(&first.rel_path).parent()?;
        if parent.as_os_str().is_empty() {
            return None;
        }
        if self
            .files
            .iter()
            .all(|file| std::path::Path::new(&file.rel_path).parent() == Some(parent))
        {
            Some(parent.to_string_lossy().into_owned())
        } else {
            None
        }
    }
}

fn file(rel_path: &str, url: &str, size_bytes: u64, sha256: &str) -> ModelFile {
    ModelFile {
        rel_path: rel_path.to_string(),
        url: url.to_string(),
        size_bytes,
        sha256: Some(sha256.to_string()),
    }
}

const WHISPER_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
const PARAKEET_BASE: &str =
    "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main";
const KOKORO_BASE: &str =
    "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0";

fn whisper(id: &str, label: &str, file_name: &str, size: u64, sha: &str) -> ModelSpec {
    ModelSpec {
        id: id.to_string(),
        label: label.to_string(),
        purpose: ModelPurpose::Stt,
        backend: ModelBackend::Whisper,
        files: vec![file(
            file_name,
            &format!("{WHISPER_BASE}/{file_name}"),
            size,
            sha,
        )],
        requires: vec!["silero-vad-5.1.2".to_string()],
    }
}

/// The canonical catalog. Order is the UI display order.
pub fn catalog() -> Vec<ModelSpec> {
    CATALOG.iter().cloned().collect()
}

pub fn spec<'a>(catalog: &'a [ModelSpec], id: &str) -> Option<&'a ModelSpec> {
    catalog.iter().find(|spec| spec.id == id)
}

pub static CATALOG: std::sync::LazyLock<Vec<ModelSpec>> = std::sync::LazyLock::new(|| {
    vec![
        ModelSpec {
            id: "silero-vad-5.1.2".to_string(),
            label: "Silero VAD 5.1.2".to_string(),
            purpose: ModelPurpose::Vad,
            backend: ModelBackend::Silero,
            // install.sh renames ggml-silero-v5.1.2.bin to the stable name used by
            // VAD_MODEL; the catalog keeps that name.
            files: vec![file(
                "ggml-silero-vad.bin",
                "https://huggingface.co/ggml-org/silero-v5.1.2/resolve/main/ggml-silero-v5.1.2.bin",
                885_098,
                "29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf",
            )],
            requires: Vec::new(),
        },
        whisper(
            "whisper-tiny",
            "Whisper tiny (multi-language)",
            "ggml-tiny.bin",
            77_691_713,
            "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        ),
        whisper(
            "whisper-small",
            "Whisper small (multi-language)",
            "ggml-small.bin",
            487_601_967,
            "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        ),
        whisper(
            "whisper-large-v3-turbo",
            "Whisper large-v3-turbo (multi-language)",
            "ggml-large-v3-turbo.bin",
            1_624_555_275,
            "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        ),
        ModelSpec {
            id: "parakeet-tdt-0.6b-v3".to_string(),
            label: "Parakeet TDT 0.6B v3 (ONNX)".to_string(),
            purpose: ModelPurpose::Stt,
            backend: ModelBackend::Parakeet,
            files: vec![
                file(
                    "parakeet-tdt-0.6b-v3-onnx/encoder-model.onnx",
                    &format!("{PARAKEET_BASE}/encoder-model.onnx"),
                    41_770_866,
                    "98a74b21b4cc0017c1e7030319a4a96f4a9506e50f0708f3a516d02a77c96bb1",
                ),
                file(
                    "parakeet-tdt-0.6b-v3-onnx/encoder-model.onnx.data",
                    &format!("{PARAKEET_BASE}/encoder-model.onnx.data"),
                    2_435_420_160,
                    "9a22d372c51455c34f13405da2520baefb7125bd16981397561423ed32d24f36",
                ),
                file(
                    "parakeet-tdt-0.6b-v3-onnx/decoder_joint-model.onnx",
                    &format!("{PARAKEET_BASE}/decoder_joint-model.onnx"),
                    72_520_893,
                    "e978ddf6688527182c10fde2eb4b83068421648985ef23f7a86be732be8706c1",
                ),
                file(
                    "parakeet-tdt-0.6b-v3-onnx/vocab.txt",
                    &format!("{PARAKEET_BASE}/vocab.txt"),
                    93_939,
                    "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
                ),
            ],
            requires: vec!["silero-vad-5.1.2".to_string()],
        },
        ModelSpec {
            id: "kokoro-v1.0".to_string(),
            label: "Kokoro v1.0 (ONNX TTS)".to_string(),
            purpose: ModelPurpose::Tts,
            backend: ModelBackend::Kokoro,
            files: vec![
                file(
                    "kokoro-v1.0.onnx",
                    &format!("{KOKORO_BASE}/kokoro-v1.0.onnx"),
                    325_532_387,
                    "7d5df8ecf7d4b1878015a32686053fd0eebe2bc377234608764cc0ef3636a6c5",
                ),
                file(
                    "voices-v1.0.bin",
                    &format!("{KOKORO_BASE}/voices-v1.0.bin"),
                    28_214_398,
                    "bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d",
                ),
            ],
            requires: Vec::new(),
        },
    ]
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_stable_ids_and_checksums() {
        let catalog = catalog();
        let mut ids: Vec<&str> = catalog.iter().map(|spec| spec.id.as_str()).collect();
        ids.sort_unstable();
        let unique = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), unique, "catalog ids must be unique");

        for spec in &catalog {
            assert!(!spec.files.is_empty(), "{} has no files", spec.id);
            for f in &spec.files {
                assert!(
                    !f.rel_path.starts_with('/'),
                    "{} path must be relative",
                    spec.id
                );
                assert!(
                    f.url.starts_with("https://"),
                    "{} url must be https",
                    spec.id
                );
                let sha = f.sha256.as_ref().expect("HTTP files need a checksum");
                assert_eq!(sha.len(), 64, "{} sha256 must be hex", spec.id);
                assert!(sha.bytes().all(|b| b.is_ascii_hexdigit()));
            }
        }
    }

    #[test]
    fn whisper_variants_require_the_vad_model() {
        for spec in catalog() {
            if spec.backend == ModelBackend::Whisper || spec.backend == ModelBackend::Parakeet {
                assert_eq!(
                    spec.requires,
                    vec!["silero-vad-5.1.2".to_string()],
                    "{} must require the VAD model",
                    spec.id
                );
            }
        }
    }

    #[test]
    fn parakeet_files_share_one_bundle_dir() {
        let catalog = catalog();
        let parakeet = spec(&catalog, "parakeet-tdt-0.6b-v3").unwrap();
        assert_eq!(
            parakeet.bundle_dir().as_deref(),
            Some("parakeet-tdt-0.6b-v3-onnx")
        );
        let kokoro = spec(&catalog, "kokoro-v1.0").unwrap();
        assert_eq!(kokoro.bundle_dir(), None);
    }
}
