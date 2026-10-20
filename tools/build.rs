//! Records the compiler that built the tools, so that a benchmark run's record names it from the run itself.
fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let version = std::process::Command::new(rustc)
        .arg("-V")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    println!("cargo:rustc-env=CORVUS_RUSTC_VERSION={}", version.trim());
    println!("cargo:rerun-if-changed=build.rs");
}
