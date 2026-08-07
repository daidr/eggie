use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=terminfo/alacritty.info");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR"));
    let database = out_dir.join("terminfo-db");
    fs::create_dir_all(&database).expect("failed to create terminfo build directory");

    let source = Path::new("terminfo/alacritty.info");
    let status = Command::new("tic")
        .args(["-x", "-e", "alacritty", "-o"])
        .arg(&database)
        .arg(source)
        .status()
        .expect("failed to execute tic while building Eggie terminfo");
    assert!(status.success(), "tic failed to compile Eggie terminfo");

    let compiled = [database.join("61/alacritty"), database.join("a/alacritty")]
        .into_iter()
        .find(|path| path.is_file())
        .expect("tic did not produce an alacritty terminfo entry");
    fs::copy(compiled, out_dir.join("alacritty.terminfo"))
        .expect("failed to stage compiled Eggie terminfo");

    // Stage the shell-integration scripts into OUT_DIR so lib.rs can embed them
    // with include_bytes!. Dotfiles (.zshenv) are staged under a plain name for
    // readability; the runtime installer writes them back under their real names.
    for (source, staged) in [
        ("shell-integration/zsh/.zshenv", "eggie-zsh-zshenv"),
        ("shell-integration/zsh/eggie-integration", "eggie-zsh-integration"),
        ("shell-integration/bash/eggie.bash", "eggie-bash"),
    ] {
        println!("cargo:rerun-if-changed={source}");
        fs::copy(source, out_dir.join(staged))
            .unwrap_or_else(|error| panic!("failed to stage {source}: {error}"));
    }
}
