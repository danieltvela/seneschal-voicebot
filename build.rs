use std::env;
use std::path::{Path, PathBuf};

fn main() {
    let profile = env::var("PROFILE").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "macos" {
        embed_macos_info_plist();
    }

    // Only add Swift RPATH when the `speech` feature is enabled
    if is_feature_enabled("speech") && target_os == "macos" {
        // The system Swift runtime ships in the dyld shared cache under
        // /usr/lib/swift. Searching there first makes @rpath/libswift_Concurrency
        // resolve to the SAME image the other Swift overlays already load,
        // avoiding duplicate objc class definitions (spurious crashes).
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
        if let Some(swift_lib_dir) = find_swift_lib_dir() {
            if swift_lib_dir != "/usr/lib/swift" {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", swift_lib_dir);
            }
            println!(
                "cargo:warning=Added Swift RPATH: {} (profile: {})",
                swift_lib_dir, profile
            );
        } else {
            println!(
                "cargo:warning=Could not find Swift library directory. \
                 The binary may fail at runtime with 'libswift_Concurrency.dylib not found'. \
                 Set SWIFT_LIB_DIR environment variable to the path containing libswift_Concurrency.dylib."
            );
        }
    }
}

fn is_feature_enabled(name: &str) -> bool {
    let var_name = format!("CARGO_FEATURE_{}", name.to_uppercase());
    env::var_os(&var_name).is_some()
}

/// Embed the daemon's own Info.plist into the Mach-O. The daemon runs as a
/// sidecar, and TCC does not resolve the containing .app's Info.plist for
/// child processes: without this section, the first microphone or
/// SFSpeechRecognizer request aborts with
/// `__TCC_CRASHING_DUE_TO_PRIVACY_VIOLATION__`.
fn embed_macos_info_plist() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let plist = Path::new(&manifest_dir).join("packaging/macos-daemon-Info.plist");
    if !plist.exists() {
        println!(
            "cargo:warning=macOS Info.plist not found at {}; TCC-protected APIs will abort",
            plist.display()
        );
        return;
    }
    println!(
        "cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );
}

fn find_swift_lib_dir() -> Option<String> {
    // Strategy 1: Environment variable override
    if let Ok(dir) = env::var("SWIFT_LIB_DIR") {
        let lib_path = Path::new(&dir).join("libswift_Concurrency.dylib");
        if lib_path.exists() {
            return Some(dir);
        }
    }

    // Strategy 2: Use xcrun to find the SDK platform path, then derive toolchain
    let output = std::process::Command::new("xcrun")
        .args(["--show-sdk-platform-path"])
        .output()
        .ok()?;
    if output.status.success() {
        let platform = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if let Some(xcode_root) = platform.split("Developer/Platforms").next() {
            for version in &["swift-5.5", "swift-5.10", "swift-macosx"] {
                let toolchain_lib = format!(
                    "{xcode_root}Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/{version}/macosx"
                );
                let lib_path = Path::new(&toolchain_lib).join("libswift_Concurrency.dylib");
                if lib_path.exists() {
                    return Some(toolchain_lib);
                }
            }
        }
    }

    // Strategy 3: Find libswift_Concurrency.dylib in /Applications
    let output = std::process::Command::new("find")
        .args([
            "/Applications",
            "-name",
            "libswift_Concurrency.dylib",
            "-path",
            "*/swift-*/macosx/*",
            "-print",
            "-quit",
        ])
        .output()
        .ok()?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty()
            && let Some(dir) = Path::new(&path).parent()
        {
            return Some(dir.to_string_lossy().to_string());
        }
    }

    // Strategy 4: Command Line Tools installs have no Xcode.app and
    // `xcrun --show-sdk-platform-path` fails, so scan the CLT swift
    // directories directly and prefer the newest match.
    let clt_root = Path::new("/Library/Developer/CommandLineTools/usr/lib");
    if let Ok(entries) = std::fs::read_dir(clt_root) {
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("swift-")
            })
            .map(|entry| entry.path().join("macosx"))
            .filter(|dir| dir.join("libswift_Concurrency.dylib").exists())
            .collect();
        candidates.sort();
        if let Some(dir) = candidates.pop() {
            return Some(dir.to_string_lossy().to_string());
        }
    }

    // Strategy 5: Check standard system location (works on older macOS)
    let system_lib = "/usr/lib/swift";
    let lib_path = Path::new(system_lib).join("libswift_Concurrency.dylib");
    if lib_path.exists() {
        return Some(system_lib.to_string());
    }

    None
}
