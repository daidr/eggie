use std::{env, fs, path::PathBuf};

fn main() {
    let themes = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../eggie-ui/assets/ghostty-themes");
    println!("cargo:rerun-if-changed={}", themes.display());
    let mut entries = Vec::new();
    for entry in fs::read_dir(themes).expect("theme directory") {
        let path = entry.expect("theme file").path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let source = fs::read_to_string(&path).expect("theme contents");
        let background = source
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "background")
                    .then(|| u32::from_str_radix(value.trim().trim_start_matches('#'), 16).ok())
                    .flatten()
            })
            .unwrap_or(0x282c34);
        let red = ((background >> 16) & 0xff) as f32 / 255.;
        let green = ((background >> 8) & 0xff) as f32 / 255.;
        let blue = (background & 0xff) as f32 / 255.;
        let dark = 0.2126 * red + 0.7152 * green + 0.0722 * blue < 0.5;
        entries.push((name, dark));
    }
    entries.sort();
    let mut code = String::from("pub const THEMES: &[(&str, bool)] = &[\n");
    for (name, dark) in entries {
        code.push_str(&format!("    ({name:?}, {dark}),\n"));
    }
    code.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("themes.rs"),
        code,
    )
    .expect("write generated theme list");
}
