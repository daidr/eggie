use crate::*;
use std::{
    collections::HashMap,
    fs,
    io::Read,
    os::unix::{fs::PermissionsExt, io::AsRawFd},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

type Cache = HashMap<String, (Instant, Arc<Vec<String>>)>;
#[derive(Default)]
pub struct CompletionEngine {
    cache: Mutex<Cache>,
    builds: Mutex<HashMap<String, Weak<Mutex<()>>>>,
}
impl CompletionEngine {
    pub fn shared() -> &'static Self {
        static ENGINE: std::sync::OnceLock<CompletionEngine> = std::sync::OnceLock::new();
        ENGINE.get_or_init(Self::default)
    }
}
impl CompletionEngine {
    fn cached(
        &self,
        key: String,
        ttl: Duration,
        build: impl FnOnce() -> Vec<String>,
    ) -> Arc<Vec<String>> {
        if let Some((when, v)) = self.cache.lock().unwrap().get(&key)
            && when.elapsed() < ttl
        {
            return v.clone();
        }
        let gate = {
            let mut builds = self.builds.lock().unwrap();
            builds.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = builds.get(&key).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(Mutex::new(()));
                builds.insert(key.clone(), Arc::downgrade(&gate));
                gate
            }
        };
        let _building = gate.lock().unwrap();
        if let Some((when, v)) = self.cache.lock().unwrap().get(&key)
            && when.elapsed() < ttl
        {
            return v.clone();
        }
        let value = Arc::new(build());
        let weight = |v: &Vec<String>| v.iter().map(|s| s.len() + 24).sum::<usize>();
        let incoming = weight(&value);
        if incoming > 4 * 1024 * 1024 {
            return value;
        }
        let mut cache = self.cache.lock().unwrap();
        cache.remove(&key);
        while cache.len() >= 64
            || cache.values().map(|(_, v)| weight(v)).sum::<usize>() + incoming > 4 * 1024 * 1024
        {
            if let Some(key) = cache
                .iter()
                .min_by_key(|(_, v)| v.0)
                .map(|(k, _)| k.clone())
            {
                cache.remove(&key);
            } else {
                break;
            }
        }
        cache.insert(key, (Instant::now(), value.clone()));
        value
    }
    pub fn complete(&self, input: &PromptInput, history: &[String]) -> Vec<CompletionItem> {
        let Some(ctx) = CompletionContext::parse(input) else {
            return vec![];
        };
        let mut items = Vec::new();
        if ctx.command_position {
            let commands = self.cached(
                format!(
                    "path:{}:{}",
                    input.search_path,
                    if std::env::split_paths(&input.search_path).any(|p| !p.is_absolute()) {
                        input.cwd.to_string_lossy().into_owned()
                    } else {
                        String::new()
                    }
                ),
                Duration::from_secs(15),
                || {
                    let mut names = Vec::new();
                    for dir in std::env::split_paths(&input.search_path) {
                        let dir = if dir.is_absolute() {
                            dir
                        } else {
                            input.cwd.join(dir)
                        };
                        if names.len() >= 20_000 {
                            break;
                        }
                        if let Ok(entries) = fs::read_dir(dir) {
                            for entry in entries.flatten().take(10_000) {
                                if entry.metadata().is_ok_and(|m| {
                                    m.is_file() && m.permissions().mode() & 0o111 != 0
                                }) && let Some(name) = entry.file_name().to_str()
                                {
                                    names.push(name.to_owned());
                                }
                            }
                        }
                    }
                    names
                },
            );
            items.extend(
                commands
                    .iter()
                    .map(|s| ctx.item(s, "", CompletionKind::Command)),
            );
            items.extend("cd echo export unset source alias unalias pwd history jobs fg bg kill exit exec read set type command test true false printf pushd popd".split_whitespace().map(|s|ctx.item(s,"",CompletionKind::Command)));
        }
        let command = ctx
            .words
            .first()
            .and_then(|v| Path::new(v).file_name())
            .and_then(|v| v.to_str())
            .unwrap_or("");
        let sub = ctx.words.get(1).map(String::as_str).unwrap_or("");
        let rules = rules(command, sub);
        if ctx.words.len() == 1 || ctx.query.starts_with('-') {
            for &(name, description) in rules {
                items.push(ctx.item(
                    name,
                    description,
                    if name.starts_with('-') {
                        CompletionKind::Option
                    } else {
                        CompletionKind::Argument
                    },
                ));
            }
        }
        if command == "git"
            && matches!(
                sub,
                "checkout"
                    | "switch"
                    | "merge"
                    | "rebase"
                    | "diff"
                    | "log"
                    | "branch"
                    | "push"
                    | "pull"
                    | "show"
                    | "reset"
            )
            && !ctx.query.starts_with('-')
        {
            let values = self.cached(
                format!("git:{}", input.cwd.display()),
                Duration::from_secs(1),
                || {
                    run(
                        input,
                        "git",
                        &[
                            "--no-optional-locks",
                            "for-each-ref",
                            "--format=%(refname:short)",
                            "refs/heads",
                            "refs/remotes",
                            "refs/tags",
                        ],
                    )
                },
            );
            items.extend(
                values
                    .iter()
                    .map(|v| ctx.item(v, "Git ref", CompletionKind::Argument)),
            );
        }
        if matches!(command, "npm" | "pnpm" | "yarn" | "bun")
            && (sub == "run" || command == "yarn" && ctx.words.len() == 1)
            && let Some(path) = find_up(&input.cwd, "package.json")
            && fs::metadata(&path).is_ok_and(|m| m.len() <= 2 * 1024 * 1024)
            && let Ok(data) = fs::read(path)
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&data)
            && let Some(scripts) = value["scripts"].as_object()
        {
            items.extend(scripts.iter().map(|(name, v)| {
                ctx.item(name, v.as_str().unwrap_or(""), CompletionKind::Argument)
            }));
        }
        if command == "ssh"
            && let Ok(data) = fs::read_to_string(input.home.join(".ssh/config"))
        {
            for line in data.lines() {
                let mut words = line.split_whitespace();
                if words.next().is_some_and(|s| s.eq_ignore_ascii_case("host")) {
                    items.extend(
                        words
                            .filter(|s| !s.contains(['*', '?', '!']))
                            .map(|v| ctx.item(v, "SSH host", CompletionKind::Argument)),
                    );
                }
            }
        }
        let directories_only = matches!(command, "cd" | "pushd" | "rmdir");
        if !ctx.query.starts_with('-') {
            items.extend(self.files(input, &ctx, directories_only));
        }
        if ctx.command_position && !input.text.trim().is_empty() {
            for line in history.iter().rev().take(200) {
                if !line.contains(['\r', '\n', '\x1b', '\x07']) {
                    items.push(CompletionItem {
                        label: line.clone(),
                        insert_text: line.clone(),
                        description: String::new(),
                        kind: CompletionKind::History,
                        replace_start: 0,
                        replace_end: input.text.len(),
                    });
                }
            }
        }
        rank(items, &ctx.query)
    }
    fn files(
        &self,
        input: &PromptInput,
        ctx: &CompletionContext,
        dirs_only: bool,
    ) -> Vec<CompletionItem> {
        let (prefix, part) = ctx
            .query
            .rsplit_once('/')
            .map(|(a, b)| (format!("{a}/"), b))
            .unwrap_or((String::new(), ctx.query.as_str()));
        let directory = if prefix.starts_with("~/") && ctx.quote.is_none() {
            input.home.join(&prefix[2..])
        } else if ctx.expand_variable
            && (prefix.starts_with("$HOME/") || prefix.starts_with("${HOME}/"))
        {
            input.home.join(prefix.split_once('/').unwrap().1)
        } else if prefix.starts_with('/') {
            PathBuf::from(&prefix)
        } else {
            input.cwd.join(&prefix)
        };
        let names = self.cached(
            format!("dir:{}", directory.display()),
            Duration::from_millis(500),
            || {
                fs::read_dir(&directory)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .take(10_000)
                    .filter_map(|entry| {
                        let name = entry.file_name().into_string().ok()?;
                        let dir = entry
                            .file_type()
                            .ok()
                            .is_some_and(|v| v.is_dir() || v.is_symlink() && entry.path().is_dir());
                        Some(format!("{name}{}", if dir { "/" } else { "" }))
                    })
                    .collect()
            },
        );
        names
            .iter()
            .filter(|name| {
                (!dirs_only || name.ends_with('/'))
                    && (!name.starts_with('.') || part.starts_with('.'))
            })
            .map(|name| {
                ctx.item(
                    &format!("{prefix}{name}"),
                    "",
                    if name.ends_with('/') {
                        CompletionKind::Directory
                    } else {
                        CompletionKind::File
                    },
                )
            })
            .collect()
    }
}
fn find_up(cwd: &Path, name: &str) -> Option<PathBuf> {
    cwd.ancestors().map(|p| p.join(name)).find(|p| p.is_file())
}
/// Read bounded output with a deadline, including providers that leave stdout open in a child.
fn run(input: &PromptInput, program: &str, args: &[&str]) -> Vec<String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(&input.cwd)
        .env("PATH", &input.search_path)
        .env("HOME", &input.home)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped());
    let Ok(mut child) = command.spawn() else {
        return vec![];
    };
    let mut stdout = child.stdout.take().unwrap();
    unsafe {
        libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
    }
    let deadline = Instant::now() + Duration::from_millis(350);
    let mut bytes = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }
        if bytes.len() > 256 * 1024 || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let _ = child.kill();
    let _ = child.wait();
    String::from_utf8_lossy(&bytes)
        .lines()
        .take(MAX_CANDIDATES)
        .filter(|s| !s.contains(['\x1b', '\x07']))
        .map(str::to_owned)
        .collect()
}
fn rules(command: &str, sub: &str) -> &'static [(&'static str, &'static str)] {
    match (command, sub) {
        ("git", "") => &[
            ("add", "Stage files"),
            ("commit", "Create a commit"),
            ("checkout", "Switch branches or restore files"),
            ("switch", "Switch branches"),
            ("status", "Show working tree status"),
            ("diff", "Compare changes"),
            ("log", "Show commits"),
            ("branch", "Manage branches"),
            ("merge", "Merge changes"),
            ("rebase", "Reapply commits"),
            ("restore", "Restore files"),
            ("stash", "Save local changes"),
            ("push", "Push commits"),
            ("pull", "Fetch and merge"),
            ("fetch", "Fetch refs"),
            ("clone", "Clone repository"),
            ("remote", "Manage remotes"),
            ("show", "Show an object"),
            ("reset", "Reset changes"),
            ("tag", "Manage tags"),
            ("worktree", "Manage working trees"),
        ],
        ("git", "commit") => &[
            ("-m", "Commit message"),
            ("--amend", "Amend last commit"),
            ("-a", "Stage tracked files"),
            ("--no-edit", "Reuse message"),
            ("--signoff", "Add sign-off"),
        ],
        ("git", "diff") => &[
            ("--stat", "Change summary"),
            ("--cached", "Staged changes"),
            ("--name-only", "File names"),
            ("--color", "Colored output"),
            ("--no-index", "Compare files"),
        ],
        ("git", "log" | "show") => &[
            ("--stat", "Change summary"),
            ("--name-only", "File names"),
            ("--oneline", "One line per commit"),
            ("--color", "Colored output"),
            ("--patch", "Show patch"),
        ],
        ("git", "checkout") => &[
            ("-b", "Create branch"),
            ("-B", "Reset branch"),
            ("--detach", "Detach HEAD"),
            ("--track", "Track remote branch"),
        ],
        ("git", "switch") => &[
            ("-c", "Create branch"),
            ("-C", "Reset branch"),
            ("--detach", "Detach HEAD"),
            ("--track", "Track remote branch"),
        ],
        ("git", "branch") => &[
            ("-d", "Delete branch"),
            ("-D", "Force delete"),
            ("-m", "Rename branch"),
            ("--list", "List branches"),
            ("--all", "All branches"),
        ],
        ("npm", "") => &[
            ("install", "Install dependencies"),
            ("uninstall", "Remove dependency"),
            ("run", "Run package script"),
            ("test", "Run tests"),
            ("init", "Initialize project"),
            ("update", "Update dependencies"),
            ("exec", "Run a command"),
            ("ci", "Clean install"),
            ("publish", "Publish package"),
        ],
        ("pnpm", "") => &[
            ("install", "Install dependencies"),
            ("add", "Add dependency"),
            ("remove", "Remove dependency"),
            ("run", "Run package script"),
            ("test", "Run tests"),
            ("init", "Initialize project"),
            ("update", "Update dependencies"),
            ("exec", "Run a command"),
            ("dlx", "Run a package"),
        ],
        ("yarn", "") => &[
            ("install", "Install dependencies"),
            ("add", "Add dependency"),
            ("remove", "Remove dependency"),
            ("run", "Run package script"),
            ("init", "Initialize project"),
            ("exec", "Run a command"),
            ("workspace", "Select workspace"),
        ],
        ("bun", "") => &[
            ("install", "Install dependencies"),
            ("add", "Add dependency"),
            ("remove", "Remove dependency"),
            ("run", "Run package script"),
            ("test", "Run tests"),
            ("build", "Build project"),
            ("init", "Initialize project"),
            ("update", "Update dependencies"),
            ("x", "Run a package"),
        ],
        ("npm", "install" | "i") => &[
            ("--save-dev", "Development dependency"),
            ("--global", "Install globally"),
            ("--omit", "Omit dependency type"),
        ],
        ("pnpm", "install" | "add") => &[
            ("-D", "Development dependency"),
            ("--global", "Install globally"),
            ("--frozen-lockfile", "Keep lockfile"),
            ("--filter", "Select workspace"),
        ],
        ("yarn", "add") => &[
            ("--dev", "Development dependency"),
            ("--exact", "Exact version"),
        ],
        ("bun", "install" | "add") => &[
            ("--dev", "Development dependency"),
            ("--global", "Install globally"),
            ("--frozen-lockfile", "Keep lockfile"),
        ],
        ("cargo", "") => &[
            ("build", "Compile"),
            ("run", "Run binary"),
            ("test", "Run tests"),
            ("check", "Check code"),
            ("clippy", "Lint code"),
            ("fmt", "Format code"),
            ("new", "Create package"),
            ("init", "Initialize package"),
            ("add", "Add dependency"),
            ("remove", "Remove dependency"),
            ("update", "Update dependencies"),
            ("doc", "Build documentation"),
            ("clean", "Clean artifacts"),
            ("install", "Install binary"),
        ],
        ("cargo", _) => &[
            ("--release", "Release profile"),
            ("-p", "Select package"),
            ("--workspace", "All packages"),
            ("--all-targets", "All targets"),
            ("--features", "Enable features"),
            ("--offline", "No network"),
            ("--locked", "Keep lockfile"),
        ],
        ("go", "") => &[
            ("build", "Compile"),
            ("run", "Run package"),
            ("test", "Run tests"),
            ("mod", "Manage modules"),
            ("get", "Add dependencies"),
            ("fmt", "Format code"),
            ("vet", "Check code"),
            ("install", "Install package"),
            ("env", "Show environment"),
        ],
        ("go", "mod") => &[
            ("tidy", "Synchronize dependencies"),
            ("init", "Initialize module"),
            ("download", "Download modules"),
            ("vendor", "Vendor dependencies"),
        ],
        ("zig", "") => &[
            ("build", "Build project"),
            ("run", "Run source"),
            ("test", "Run tests"),
            ("fmt", "Format source"),
            ("init", "Initialize project"),
            ("build-exe", "Build executable"),
            ("build-lib", "Build library"),
        ],
        ("node", _) => &[
            ("--inspect", "Enable debugger"),
            ("--inspect-brk", "Break before user code"),
            ("--watch", "Watch files"),
            ("--test", "Run tests"),
            ("--eval", "Evaluate source"),
            ("--input-type", "Select module type"),
        ],
        ("python" | "python3", _) => &[
            ("-m", "Run module"),
            ("-c", "Run source"),
            ("-i", "Interactive mode"),
            ("-u", "Unbuffered output"),
            ("-V", "Show version"),
        ],
        ("docker", "") => &[
            ("run", "Run container"),
            ("build", "Build image"),
            ("compose", "Compose services"),
            ("ps", "List containers"),
            ("images", "List images"),
            ("exec", "Run in container"),
            ("logs", "Container logs"),
            ("pull", "Pull image"),
            ("push", "Push image"),
            ("stop", "Stop container"),
        ],
        ("docker", "compose") => &[
            ("up", "Start services"),
            ("down", "Stop services"),
            ("logs", "Show logs"),
            ("build", "Build services"),
            ("exec", "Run command"),
            ("ps", "List services"),
        ],
        ("codex", _) => &[
            ("exec", "Run noninteractively"),
            ("resume", "Resume session"),
            ("review", "Review changes"),
            ("--model", "Choose model"),
            ("--sandbox", "Choose sandbox"),
            ("--cd", "Working directory"),
        ],
        ("claude", _) => &[
            ("--continue", "Continue session"),
            ("--resume", "Resume session"),
            ("--model", "Choose model"),
            ("--print", "Print response"),
            ("--add-dir", "Additional directory"),
        ],
        ("opencode", _) => &[
            ("run", "Run prompt"),
            ("serve", "Start server"),
            ("models", "List models"),
            ("--model", "Choose model"),
            ("--continue", "Continue session"),
        ],
        _ => &[],
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_queries_share_one_cache_fill() {
        use std::sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        };
        let engine = Arc::new(CompletionEngine::default());
        let ready = Arc::new(Barrier::new(8));
        let fills = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let engine = Arc::clone(&engine);
                let ready = Arc::clone(&ready);
                let fills = Arc::clone(&fills);
                std::thread::spawn(move || {
                    ready.wait();
                    engine.cached("shared".into(), Duration::from_secs(15), || {
                        fills.fetch_add(1, Ordering::Relaxed);
                        std::thread::sleep(Duration::from_millis(20));
                        vec!["git".into()]
                    })
                })
            })
            .collect();
        let values: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(fills.load(Ordering::Relaxed), 1);
        assert!(values.iter().all(|value| Arc::ptr_eq(value, &values[0])));
    }
    #[test]
    fn report_unicode_and_controls() {
        let i = PromptInput::from_report("zsh;2;/tmp;/bin;/home;1;中%3Ba%0Ab").unwrap();
        assert_eq!(i.cursor, "中;".len());
        assert_eq!(i.text, "中;a\nb");
        assert!(PromptInput::from_report("zsh;99;/;/bin;/;1;x").is_none());
    }
    #[test]
    fn rank_prefix_before_subsequence() {
        let c = CompletionContext {
            expand_variable: false,
            words: vec![],
            query: String::new(),
            start: 0,
            end: 0,
            quote: None,
            command_position: true,
        };
        let v = rank(
            vec![
                c.item("foo-bar", "", CompletionKind::Command),
                c.item("fb", "", CompletionKind::Command),
            ],
            "fb",
        );
        assert_eq!(v[0].label, "fb");
    }
    #[test]
    fn files_and_scripts() {
        let dir =
            std::env::temp_dir().join(format!("eggie-completion-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a file.txt"), "").unwrap();
        fs::create_dir_all(dir.join("folder")).unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"start":"node app.js"}}"#,
        )
        .unwrap();
        let engine = CompletionEngine::default();
        let mut input = PromptInput {
            text: "cat a".into(),
            cursor: 5,
            cwd: dir.clone(),
            ..Default::default()
        };
        let items = engine.complete(&input, &[]);
        assert!(items.iter().any(|v| v.insert_text == "a\\ file.txt"));
        input.text = "pnpm run s".into();
        input.cursor = input.text.len();
        assert!(
            engine
                .complete(&input, &[])
                .iter()
                .any(|v| v.label == "start" && v.description == "node app.js")
        );
        input.text = "cd ".into();
        input.cursor = 3;
        assert!(
            engine
                .complete(&input, &[])
                .iter()
                .all(|v| v.kind == CompletionKind::Directory)
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
