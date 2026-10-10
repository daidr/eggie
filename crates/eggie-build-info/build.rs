use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let workspace = manifest.join("../..");
    let roots = [
        workspace.join("Cargo.toml"),
        workspace.join("Cargo.lock"),
        workspace.join("crates"),
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

    // Build date shown in the settings "About" panel. Reproducible when
    // SOURCE_DATE_EPOCH is set (CI / packagers); otherwise the wall clock at the
    // moment build.rs actually re-runs — i.e. when one of the `rerun-if-changed`
    // inputs above changes. It intentionally does NOT tick on a no-op rebuild,
    // which matches the meaning of a "build date": the last real recompile.
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let epoch: u64 = env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before the unix epoch")
                .as_secs()
        });
    let (year, month, day) = civil_from_days((epoch / 86_400) as i64);
    println!("cargo:rustc-env=EGGIE_BUILD_DATE={year:04}-{month:02}-{day:02}");
}

/// Convert a count of days since 1970-01-01 into a proleptic-Gregorian
/// (year, month, day) in UTC. Howard Hinnant's public-domain `civil_from_days`
/// algorithm — avoids pulling a date crate into the build just to format a day.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_file() {
        files.push(path.to_path_buf());
        return;
    }
    println!("cargo:rerun-if-changed={}", path.display());
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
