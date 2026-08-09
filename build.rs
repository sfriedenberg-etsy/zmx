use std::process::Command;

// Burn the version and commit into the binary. ZMX_VERSION/ZMX_COMMIT env
// vars (set by package.nix for release builds) win; otherwise fall back to
// the Cargo package version and `git rev-parse` (sandboxed source trees with
// no .git get "unknown").
fn main() {
    println!("cargo:rerun-if-env-changed=ZMX_VERSION");
    println!("cargo:rerun-if-env-changed=ZMX_COMMIT");

    let version = std::env::var("ZMX_VERSION")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=ZMX_BUILD_VERSION={version}");

    let commit = std::env::var("ZMX_COMMIT").unwrap_or_else(|_| {
        Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    });
    println!("cargo:rustc-env=ZMX_BUILD_COMMIT={commit}");
}
