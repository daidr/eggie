use std::{env, path::PathBuf, process::Command};

fn main() {
    let source = "src/metal_terminal/terminal.metal";
    println!("cargo:rerun-if-changed={source}");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let air = output.join("terminal.air");
    let library = output.join("terminal.metallib");
    for args in [
        vec![
            "-sdk",
            "macosx",
            "metal",
            "-mmacosx-version-min=10.15.7",
            "-MO",
            "-c",
            source,
            "-o",
            air.to_str().unwrap(),
        ],
        vec![
            "-sdk",
            "macosx",
            "metallib",
            air.to_str().unwrap(),
            "-o",
            library.to_str().unwrap(),
        ],
    ] {
        let result = Command::new("xcrun")
            .args(args)
            .output()
            .expect("Metal compiler requires Xcode and its Metal toolchain");
        assert!(
            result.status.success(),
            "terminal Metal compilation failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
