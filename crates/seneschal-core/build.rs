//! Compiles a tiny C shim that installs libasound's global error handler, so
//! ALSA diagnostics (e.g. `pcm_dsnoop ... unable to open slave`) go to
//! `tracing` instead of spamming stderr. Only built on Linux (libasound).

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }

    let alsa = pkg_config::Config::new().probe("alsa").expect(
        "pkg-config could not find `alsa`; install libasound2-dev (Debian/Ubuntu: \
         sudo apt-get install libasound2-dev)",
    );

    let mut build = cc::Build::new();
    build.file("native/alsa_silencer.c").warnings(false);
    for inc in &alsa.include_paths {
        build.include(inc);
    }
    build.compile("seneschal_alsa_silencer");

    println!("cargo:rerun-if-changed=native/alsa_silencer.c");
}
