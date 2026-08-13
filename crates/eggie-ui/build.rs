use std::{
    env, fs,
    path::{Path, PathBuf},
};

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let workspace = manifest.join("../..");
    generate_ghostty_theme_sources(&manifest);
    let roots = [
        workspace.join("Cargo.toml"),
        workspace.join("Cargo.lock"),
        workspace.join("crates/eggie-ui/Cargo.toml"),
        workspace.join("crates/eggie-ui/src"),
        workspace.join("crates/eggie-ui/assets/ghostty-themes"),
        workspace.join("crates/eggie-daemon/Cargo.toml"),
        workspace.join("crates/eggie-daemon/src"),
        workspace.join("crates/eggie-protocol/Cargo.toml"),
        workspace.join("crates/eggie-protocol/src"),
        workspace.join("crates/eggie-domain/Cargo.toml"),
        workspace.join("crates/eggie-domain/src"),
        workspace.join("vendor/alacritty/alacritty_terminal/Cargo.toml"),
        workspace.join("vendor/alacritty/alacritty_terminal/src"),
    ];

    let mut files = Vec::new();
    for root in roots {
        collect_files(&root, &mut files);
    }
    files.sort();

    let mut hash = FNV_OFFSET;
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        hash_bytes(
            &mut hash,
            path.strip_prefix(&workspace)
                .unwrap_or(&path)
                .as_os_str()
                .as_encoded_bytes(),
        );
        hash_bytes(
            &mut hash,
            &fs::read(&path).unwrap_or_else(|error| {
                panic!("failed to read build-id input {}: {error}", path.display())
            }),
        );
    }
    // Debug builds identify themselves by a content hash so any recompile swaps
    // in a fresh daemon (see handshake_accepted). Release builds use a stable,
    // version-derived id so in-place updates with the same protocol version can
    // keep the existing daemon alive.
    let profile = env::var("PROFILE").unwrap_or_default();
    let build_id = if profile == "release" {
        format!("release-{}", env!("CARGO_PKG_VERSION"))
    } else {
        format!("dev-{hash:016x}")
    };
    println!("cargo:rustc-env=EGGIE_BUILD_ID={build_id}");
}

fn generate_ghostty_theme_sources(manifest: &Path) {
    let themes = manifest.join("assets/ghostty-themes");
    println!("cargo:rerun-if-changed={}", themes.display());
    let mut names = fs::read_dir(&themes)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", themes.display()))
        .filter_map(|entry| {
            let path = entry.expect("theme directory entry").path();
            path.is_file().then(|| {
                path.file_name()
                    .expect("theme file has a name")
                    .to_string_lossy()
                    .into_owned()
            })
        })
        .collect::<Vec<_>>();
    names.sort();

    let mut source = String::from("pub const GHOSTTY_THEME_SOURCES: &[(&str, &str)] = &[\n");
    for name in names {
        source.push_str(&format!(
            "    ({name:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/assets/ghostty-themes/\", {name:?}))),\n"
        ));
    }
    source.push_str("];\n");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("build output directory"))
        .join("ghostty_themes.rs");
    fs::write(&output, source)
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", output.display()));
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_file() {
        files.push(path.to_path_buf());
        return;
    }
    let mut children = fs::read_dir(path)
        .unwrap_or_else(|error| {
            panic!(
                "failed to read build-id directory {}: {error}",
                path.display()
            )
        })
        .map(|entry| entry.expect("build-id directory entry").path())
        .collect::<Vec<_>>();
    children.sort();
    for child in children {
        collect_files(&child, files);
    }
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}
