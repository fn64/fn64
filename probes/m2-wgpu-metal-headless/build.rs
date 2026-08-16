use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rustc-env=FN64_PROBE_TARGET={}",
        env::var("TARGET").unwrap()
    );

    let output = Command::new(env::var("RUSTC").unwrap())
        .arg("-Vv")
        .output()
        .expect("run rustc -Vv");
    assert!(output.status.success(), "rustc -Vv failed");
    let version = String::from_utf8(output.stdout).expect("rustc -Vv was not UTF-8");
    let release = version
        .lines()
        .find_map(|line| line.strip_prefix("release: "))
        .expect("rustc -Vv omitted release");
    let commit_hash = version
        .lines()
        .find_map(|line| line.strip_prefix("commit-hash: "))
        .expect("rustc -Vv omitted commit hash");
    println!("cargo:rustc-env=FN64_PROBE_RUSTC_RELEASE={release}");
    println!("cargo:rustc-env=FN64_PROBE_RUSTC_COMMIT_HASH={commit_hash}");
}
