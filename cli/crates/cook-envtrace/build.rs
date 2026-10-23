//! Builds the getenv recorder (Linux only: it is an LD_PRELOAD library) and
//! names the variable it writes its output path from, once, for both the C
//! source (`-DOUT_VAR`) and `lib.rs`.

use std::path::PathBuf;

const OUT_VAR: &str = "COOK_ENVTRACE_OUT";

fn main() {
    println!("cargo:rerun-if-changed=src/envtrace.c");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(envtrace_built)");
    println!("cargo:rustc-env=COOK_ENVTRACE_OUT_VAR={OUT_VAR}");
    // The target's OS, not the build host's: `cfg!` here would describe the host.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("libcook_envtrace.so");
    let cc = cc::Build::new().get_compiler();
    // -ldl: release binaries are built against glibc 2.31, where dlsym lives there.
    let status = cc
        .to_command()
        .args(["-shared", "-fPIC", "-O2"])
        .arg(format!("-DOUT_VAR=\"{OUT_VAR}\""))
        .arg("-o")
        .arg(&out)
        .arg("src/envtrace.c")
        .arg("-ldl")
        .status()
        .expect("run the C compiler");
    assert!(status.success(), "compiling src/envtrace.c failed");
    println!("cargo:rustc-cfg=envtrace_built");
}
