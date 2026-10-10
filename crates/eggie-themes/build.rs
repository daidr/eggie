use std::{
    env, fs,
    path::{Path, PathBuf},
};
fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    generate_ghostty_theme_sources(&manifest);
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
