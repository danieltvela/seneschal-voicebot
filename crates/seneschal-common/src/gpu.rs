//! GPU discovery and per-model device assignment (issue #14: multi-GPU hosts).
//!
//! Each local ONNX model (Silero VAD, Parakeet STT, Kokoro TTS) can be pinned
//! to a specific GPU. With `GPU_AUTO` (the default) the models are distributed
//! round-robin over the GPUs with the most free VRAM, so a multi-GPU host uses
//! all of them instead of piling everything on device 0.
//!
//! Configuration precedence for each model:
//!   1. Explicit per-model value (`VAD_GPU`, `PARAKEET_GPU`, `TTS_GPU`):
//!      a GPU index (`0`, `1`, ...) or `cpu`.
//!   2. `GPU_AUTO=1` (default): auto-assign a distinct GPU.
//!   3. CPU.
//!
//! Whisper is not device-pinned: upstream `whisper-cpp-plus` has no per-context
//! device API, so it uses whisper.cpp's default (GPU 0 of the visible devices
//! when built with `cuda`). Use `CUDA_VISIBLE_DEVICES` to reorder devices.
//!
//! Device indices refer to the devices *visible to this process*, i.e. after
//! `CUDA_VISIBLE_DEVICES` filtering.

use std::fmt;
use std::process::Command;

use tracing::{info, warn};

use crate::config::Config;

/// A GPU as reported by the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuDevice {
    /// Device index as used by CUDA / ONNX Runtime.
    pub index: i32,
    pub name: String,
    pub total_mb: u64,
    pub used_mb: u64,
}

impl GpuDevice {
    pub fn free_mb(&self) -> u64 {
        self.total_mb.saturating_sub(self.used_mb)
    }
}

impl fmt::Display for GpuDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GPU {} ({}, {} MiB free / {} MiB total)",
            self.index,
            self.name,
            self.free_mb(),
            self.total_mb
        )
    }
}

/// Raw per-model configuration value, before auto-assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceChoice {
    /// No explicit value: auto-assign when `gpu_auto` is on, else CPU.
    Auto,
    /// Force CPU.
    Cpu,
    /// Force a GPU index.
    Index(i32),
}

impl DeviceChoice {
    /// Parse an env/TOML value. `None` for unknown values (caller warns).
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("auto") {
            return Some(Self::Auto);
        }
        if value.eq_ignore_ascii_case("cpu") || value.eq_ignore_ascii_case("none") {
            return Some(Self::Cpu);
        }
        value.parse::<i32>().ok().map(|index| {
            if index < 0 {
                Self::Cpu
            } else {
                Self::Index(index)
            }
        })
    }
}

impl fmt::Display for DeviceChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Cpu => write!(f, "cpu"),
            Self::Index(i) => write!(f, "{i}"),
        }
    }
}

/// Resolved device for a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Cpu,
    Gpu(i32),
}

impl Device {
    /// `Some(index)` when the model runs on a GPU.
    pub fn index(&self) -> Option<i32> {
        match self {
            Self::Cpu => None,
            Self::Gpu(index) => Some(*index),
        }
    }

    pub fn is_cpu(&self) -> bool {
        matches!(self, Self::Cpu)
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cpu => write!(f, "cpu"),
            Self::Gpu(index) => write!(f, "gpu{index}"),
        }
    }
}

/// Device assignment for every locally-loaded model.
///
/// Whisper itself is not listed: upstream `whisper-cpp-plus` always uses
/// whisper.cpp's default device (GPU 0 of the visible devices when built with
/// `cuda`). Use `CUDA_VISIBLE_DEVICES` to re-order/filter devices for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuAssignments {
    /// Silero VAD context (shared by whisper/parakeet/speech providers).
    pub vad: Device,
    /// Parakeet ONNX encoder/decoder (ONNX Runtime).
    pub parakeet: Device,
    /// Kokoro ONNX TTS (ONNX Runtime).
    pub tts: Device,
}

impl GpuAssignments {
    /// All-CPU assignment (used when discovery is unavailable).
    pub fn cpu() -> Self {
        Self {
            vad: Device::Cpu,
            parakeet: Device::Cpu,
            tts: Device::Cpu,
        }
    }
}

/// Discover the GPUs visible to this process.
///
/// - macOS: one synthetic device (Metal/CoreML single-GPU).
/// - Linux/Windows: `nvidia-smi` query; empty when unavailable.
pub fn discover_devices() -> Vec<GpuDevice> {
    #[cfg(target_os = "macos")]
    {
        vec![GpuDevice {
            index: 0,
            name: "Apple GPU (Metal/CoreML)".to_string(),
            total_mb: 0,
            used_mb: 0,
        }]
    }

    #[cfg(not(target_os = "macos"))]
    {
        discover_nvidia()
    }
}

#[cfg(not(target_os = "macos"))]
fn discover_nvidia() -> Vec<GpuDevice> {
    let output = match Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,name,memory.total,memory.used",
            "--format=csv,noheader,nounits",
        ])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };

    if !output.status.success() {
        return Vec::new();
    }

    parse_nvidia_smi_csv(&String::from_utf8_lossy(&output.stdout))
}

/// Parse `nvidia-smi --query-gpu=index,name,memory.total,memory.used
/// --format=csv,noheader,nounits` output. Kept public for tests.
pub fn parse_nvidia_smi_csv(raw: &str) -> Vec<GpuDevice> {
    let mut devices = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("index") {
            continue;
        }
        // `name` may contain commas in theory; split into exactly 4 fields.
        let mut fields = line.splitn(4, ',');
        let (Some(index), Some(name), Some(total), Some(used)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(index), Ok(total_mb), Ok(used_mb)) = (
            index.trim().parse::<i32>(),
            total.trim().parse::<u64>(),
            used.trim().parse::<u64>(),
        ) else {
            continue;
        };
        devices.push(GpuDevice {
            index,
            name: name.trim().to_string(),
            total_mb,
            used_mb,
        });
    }
    devices
}

/// Resolve the assignment for every model from config + discovered devices.
///
/// Pure function: the device list is injected so it can be unit-tested.
pub fn resolve_assignments(config: &Config, devices: &[GpuDevice]) -> GpuAssignments {
    if !config.gpu_auto
        && config.vad_gpu.is_none()
        && config.parakeet_gpu.is_none()
        && config.tts_gpu.is_none()
    {
        return GpuAssignments::cpu();
    }

    // Round-robin order: GPUs with the most free VRAM first, index as tiebreak.
    let mut auto_order: Vec<i32> = devices.iter().map(|d| d.index).collect();
    auto_order.sort_by_key(|index| {
        let device = devices.iter().find(|d| d.index == *index);
        let free = device.map(|d| d.free_mb()).unwrap_or(0);
        (std::cmp::Reverse(free), *index)
    });

    let mut next = 0usize;
    let mut resolve = |label: &str, raw: Option<&str>| -> Device {
        let choice = match raw {
            Some(value) => match DeviceChoice::parse(value) {
                Some(choice) => choice,
                None => {
                    warn!(
                        target: "gpu",
                        "{label}_GPU value '{value}' is not a device index or 'cpu'; ignoring"
                    );
                    DeviceChoice::Auto
                }
            },
            None => DeviceChoice::Auto,
        };

        match choice {
            DeviceChoice::Cpu => Device::Cpu,
            DeviceChoice::Index(index) => {
                if devices.iter().any(|d| d.index == index) {
                    Device::Gpu(index)
                } else {
                    warn!(
                        target: "gpu",
                        "{label} requested GPU {index}, but only {} GPU(s) are visible; falling back to CPU",
                        devices.len()
                    );
                    Device::Cpu
                }
            }
            DeviceChoice::Auto => {
                if config.gpu_auto && !auto_order.is_empty() {
                    let index = auto_order[next % auto_order.len()];
                    next += 1;
                    Device::Gpu(index)
                } else {
                    Device::Cpu
                }
            }
        }
    };

    GpuAssignments {
        vad: resolve("VAD", config.vad_gpu.as_deref()),
        parakeet: resolve("PARAKEET", config.parakeet_gpu.as_deref()),
        tts: resolve("TTS", config.tts_gpu.as_deref()),
    }
}

/// Log the discovered GPUs and the resolved assignment.
pub fn log_summary(devices: &[GpuDevice], assignments: &GpuAssignments) {
    if devices.is_empty() {
        info!(target: "gpu", "No GPUs detected; all local models run on CPU");
    } else {
        for device in devices {
            info!(target: "gpu", "Detected {device}");
        }
    }
    info!(
        target: "gpu",
        "Model assignment: vad={}, parakeet={}, kokoro(tts)={} (whisper uses whisper.cpp device 0)",
        assignments.vad,
        assignments.parakeet,
        assignments.tts
    );
}

/// Print the GPU section of `--list-devices`.
pub fn print_devices(devices: &[GpuDevice], assignments: &GpuAssignments) {
    println!();
    println!("GPUs (visible to this process):");
    if devices.is_empty() {
        println!("  (none detected — VAD/TTS/Parakeet run on CPU)");
    } else {
        for device in devices {
            println!("  {device}");
        }
    }
    println!();
    println!("Resolved model assignment:");
    println!("  silero VAD:          {}", assignments.vad);
    println!("  parakeet (STT):      {}", assignments.parakeet);
    println!("  kokoro (TTS):        {}", assignments.tts);
    println!(
        "  whisper (STT):       whisper.cpp default device (GPU 0); use CUDA_VISIBLE_DEVICES to reorder"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Vec<GpuDevice> {
        vec![
            GpuDevice {
                index: 0,
                name: "RTX A".to_string(),
                total_mb: 24_000,
                used_mb: 20_000, // 4 000 free
            },
            GpuDevice {
                index: 1,
                name: "RTX B".to_string(),
                total_mb: 24_000,
                used_mb: 1_000, // 23 000 free
            },
        ]
    }

    #[test]
    fn parses_nvidia_smi_csv() {
        let raw = "0, NVIDIA RTX PRO 6000 Blackwell Max-Q Workstation Edition, 97887, 75529\n\
                   1, NVIDIA RTX PRO 6000 Blackwell Max-Q Workstation Edition, 97887, 92144\n";
        let parsed = parse_nvidia_smi_csv(raw);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].index, 1);
        assert_eq!(parsed[1].total_mb, 97_887);
        assert_eq!(parsed[1].used_mb, 92_144);
        assert_eq!(parsed[1].free_mb(), 5_743);
    }

    #[test]
    fn parse_skips_garbage_lines() {
        let parsed = parse_nvidia_smi_csv("not,a,valid,line\n\n");
        assert!(parsed.is_empty());
    }

    #[test]
    fn device_choice_parses_values() {
        assert_eq!(DeviceChoice::parse("auto"), Some(DeviceChoice::Auto));
        assert_eq!(DeviceChoice::parse(""), Some(DeviceChoice::Auto));
        assert_eq!(DeviceChoice::parse("CPU"), Some(DeviceChoice::Cpu));
        assert_eq!(DeviceChoice::parse("none"), Some(DeviceChoice::Cpu));
        assert_eq!(DeviceChoice::parse("-1"), Some(DeviceChoice::Cpu));
        assert_eq!(DeviceChoice::parse(" 1 "), Some(DeviceChoice::Index(1)));
        assert_eq!(DeviceChoice::parse("banana"), None);
    }

    #[test]
    fn auto_assigns_round_robin_starting_with_freest_gpu() {
        let config = Config::test_defaults();
        let assignments = resolve_assignments(&config, &devices());
        // GPU 1 has more free memory, so it goes first.
        assert_eq!(assignments.vad, Device::Gpu(1));
        assert_eq!(assignments.parakeet, Device::Gpu(0));
        assert_eq!(assignments.tts, Device::Gpu(1));
    }

    #[test]
    fn explicit_choice_wins_over_auto() {
        let mut config = Config::test_defaults();
        config.vad_gpu = Some("0".to_string());
        config.tts_gpu = Some("cpu".to_string());
        let assignments = resolve_assignments(&config, &devices());
        assert_eq!(assignments.vad, Device::Gpu(0));
        assert_eq!(assignments.tts, Device::Cpu);
        // The remaining model still auto-assigns.
        assert!(matches!(assignments.parakeet, Device::Gpu(_)));
    }

    #[test]
    fn auto_off_without_overrides_is_all_cpu() {
        let mut config = Config::test_defaults();
        config.gpu_auto = false;
        assert_eq!(
            resolve_assignments(&config, &devices()),
            GpuAssignments::cpu()
        );
    }

    #[test]
    fn auto_off_with_one_override_only_that_model_uses_gpu() {
        let mut config = Config::test_defaults();
        config.gpu_auto = false;
        config.tts_gpu = Some("1".to_string());
        let assignments = resolve_assignments(&config, &devices());
        assert_eq!(assignments.tts, Device::Gpu(1));
        assert_eq!(assignments.vad, Device::Cpu);
        assert_eq!(assignments.parakeet, Device::Cpu);
    }

    #[test]
    fn no_devices_means_cpu() {
        let config = Config::test_defaults();
        assert_eq!(resolve_assignments(&config, &[]), GpuAssignments::cpu());
    }

    #[test]
    fn out_of_range_device_falls_back_to_cpu() {
        let mut config = Config::test_defaults();
        config.vad_gpu = Some("7".to_string());
        let assignments = resolve_assignments(&config, &devices());
        assert_eq!(assignments.vad, Device::Cpu);
    }

    #[test]
    fn invalid_value_falls_back_to_auto() {
        let mut config = Config::test_defaults();
        config.vad_gpu = Some("banana".to_string());
        let assignments = resolve_assignments(&config, &devices());
        assert!(matches!(assignments.vad, Device::Gpu(_)));
    }
}
