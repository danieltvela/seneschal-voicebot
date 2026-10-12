use std::env;
use std::path::Path;

fn main() {
    let profile = env::var("PROFILE").unwrap_or_default();

    // Only add Swift RPATH when the `speech` feature is enabled
    if is_feature_enabled("speech") {
        if let Some(swift_lib_dir) = find_swift_lib_dir() {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", swift_lib_dir);
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

    // Strategy 4: Check standard system location (works on older macOS)
    let system_lib = "/usr/lib/swift";
    let lib_path = Path::new(system_lib).join("libswift_Concurrency.dylib");
    if lib_path.exists() {
        return Some(system_lib.to_string());
    }

    None
}
