//! Records what built the tools, so that a benchmark run's record names it from the run itself: the compiler, and
//! the versions of the engines from the workspace's Cargo.lock.
fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let version = std::process::Command::new(rustc)
        .arg("-V")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    println!("cargo:rustc-env=CORVUS_RUSTC_VERSION={}", version.trim());
    let lock = std::fs::read_to_string("../Cargo.lock").unwrap_or_default();
    for (name, var) in
        [("corvus-regex-ecma262", "CORVUS_VERSION"), ("regex", "REGEX_VERSION"), ("regress", "REGRESS_VERSION")]
    {
        let mut found = String::new();
        let mut lines = lock.lines();
        while let Some(line) = lines.next() {
            if line == format!("name = \"{name}\"") {
                if let Some(v) = lines.next().and_then(|l| l.strip_prefix("version = \"")) {
                    found = v.trim_end_matches('"').to_string();
                }
                break;
            }
        }
        println!("cargo:rustc-env={var}={found}");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../Cargo.lock");
}
