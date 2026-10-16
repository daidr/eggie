//! Discover literal aliases without evaluating SSH config commands; OpenSSH resolves effective values.
use super::*;
use std::ffi::{CStr, CString};

fn words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escape = false;
    for c in line.chars() {
        if escape {
            word.push(c);
            escape = false;
            continue;
        }
        if c == '\\' {
            escape = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
            continue;
        }
        if c == '#' {
            break;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
        } else if c.is_whitespace() || (c == '=' && words.is_empty()) {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}
fn expand(pattern: &Path) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let Ok(pattern) = CString::new(pattern.as_os_str().as_bytes()) else {
        return Vec::new();
    };
    unsafe {
        let mut matches: libc::glob_t = std::mem::zeroed();
        let status = libc::glob(pattern.as_ptr(), 0, None, &mut matches);
        let paths = if status == 0 {
            (0..matches.gl_pathc.min(256))
                .map(|i| {
                    PathBuf::from(std::ffi::OsStr::from_bytes(
                        CStr::from_ptr(*matches.gl_pathv.add(i)).to_bytes(),
                    ))
                })
                .collect()
        } else {
            Vec::new()
        };
        libc::globfree(&mut matches);
        paths
    }
}
fn visit(
    path: &Path,
    base: &Path,
    home: &Path,
    seen: &mut HashSet<PathBuf>,
    hosts: &mut HashSet<String>,
    budget: &mut usize,
) -> Result<()> {
    if *budget == 0 || seen.len() >= 64 {
        return Ok(());
    }
    let Ok(path) = path.canonicalize() else {
        return Ok(());
    };
    if !seen.insert(path.clone()) {
        return Ok(());
    }
    let mut file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(_) => return Ok(()),
    };
    let mut text = String::new();
    (&mut file)
        .take((*budget).min(1024 * 1024) as u64)
        .read_to_string(&mut text)?;
    *budget = budget.saturating_sub(text.len());
    for line in text.lines() {
        let tokens = words(line);
        let Some(key) = tokens.first() else {
            continue;
        };
        if key.eq_ignore_ascii_case("Host") {
            for host in tokens.iter().skip(1) {
                if hosts.len() < 1024
                    && host.len() <= 255
                    && !host.contains(['*', '?', '!', '%', '[', ']'])
                    && RemoteTarget::new(host.clone()).is_usable()
                {
                    hosts.insert(host.clone());
                }
            }
        } else if key.eq_ignore_ascii_case("Include") {
            for include in tokens.iter().skip(1) {
                let pattern = if let Some(rest) = include.strip_prefix("~/") {
                    home.join(rest)
                } else if Path::new(include).is_absolute() {
                    PathBuf::from(include)
                } else {
                    base.join(include)
                };
                for path in expand(&pattern) {
                    visit(&path, base, home, seen, hosts, budget)?;
                }
            }
        }
    }
    Ok(())
}
pub(crate) fn hosts() -> Result<Vec<String>> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is unavailable")?);
    let mut hosts = HashSet::new();
    let mut seen = HashSet::new();
    let mut budget = 2 * 1024 * 1024;
    visit(
        &home.join(".ssh/config"),
        &home.join(".ssh"),
        &home,
        &mut seen,
        &mut hosts,
        &mut budget,
    )?;
    visit(
        Path::new("/etc/ssh/ssh_config"),
        Path::new("/etc/ssh"),
        &home,
        &mut seen,
        &mut hosts,
        &mut budget,
    )?;
    let mut hosts: Vec<_> = hosts.into_iter().collect();
    hosts.sort_by_key(|host| host.to_lowercase());
    Ok(hosts)
}
pub(crate) fn resolve(target: RemoteTarget) -> Result<eggie_protocol::SshEffectiveConfig> {
    resolve_using_config(target, None)
}
fn resolve_using_config(
    target: RemoteTarget,
    config: Option<&Path>,
) -> Result<eggie_protocol::SshEffectiveConfig> {
    anyhow::ensure!(target.is_usable(), "Invalid SSH target");
    let mut command = std::process::Command::new("/usr/bin/ssh");
    command.arg("-G");
    if let Some(config) = config {
        command.arg("-F").arg(config);
    }
    if let Some(user) = &target.user {
        command.args(["-l", user]);
    }
    if let Some(port) = target.port {
        command.args(["-p", &port.to_string()]);
    }
    command.arg(&target.host);
    let result = command::run(command, Duration::from_secs(3))?;
    anyhow::ensure!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    let output = String::from_utf8(result.stdout)?;
    let values = |key: &str| {
        output
            .lines()
            .filter_map(move |line| {
                line.strip_prefix(key)
                    .and_then(|line| line.strip_prefix(' '))
            })
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    Ok(eggie_protocol::SshEffectiveConfig {
        hostname: values("hostname").into_iter().next().unwrap_or_default(),
        user: values("user").into_iter().next().unwrap_or_default(),
        port: values("port")
            .into_iter()
            .next()
            .and_then(|port| port.parse().ok())
            .unwrap_or(22),
        identity_files: values("identityfile"),
        proxy_jump: values("proxyjump")
            .into_iter()
            .next()
            .filter(|v| v != "none"),
        uses_proxy_command: values("proxycommand").first().is_some_and(|v| v != "none"),
    })
}

pub(crate) fn diagnose(text: &str) -> Option<eggie_protocol::SshFailureKind> {
    use eggie_protocol::SshFailureKind::*;
    let text = text.to_ascii_lowercase();
    [
        (
            [
                "host key verification failed",
                "remote host identification has changed",
            ]
            .as_slice(),
            HostKey,
        ),
        (
            [
                "permission denied",
                "authentication failed",
                "too many authentication failures",
            ]
            .as_slice(),
            Authentication,
        ),
        (
            [
                "could not resolve hostname",
                "name or service not known",
                "nodename nor servname",
            ]
            .as_slice(),
            NameResolution,
        ),
        (
            ["connection timed out", "operation timed out"].as_slice(),
            Timeout,
        ),
        (["connection refused"].as_slice(), Refused),
        (
            [
                "no route to host",
                "network is unreachable",
                "connection reset",
                "broken pipe",
                "connection closed",
            ]
            .as_slice(),
            Network,
        ),
    ]
    .into_iter()
    .find_map(|(patterns, kind)| patterns.iter().any(|p| text.contains(p)).then_some(kind))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn openssh_resolves_aliases_and_explicit_overrides_without_connecting() {
        let path = std::env::temp_dir().join(format!("ssh-effective-{}", SessionId::new_v4()));
        std::fs::write(&path,"Host eggie-fixture\n HostName 192.0.2.1\n User fixture\n Port 2222\n ProxyJump gateway\n IdentityFile ~/.ssh/eggie-fixture\n").unwrap();
        let config =
            resolve_using_config(RemoteTarget::new("eggie-fixture".into()), Some(&path)).unwrap();
        assert_eq!(config.hostname, "192.0.2.1");
        assert_eq!(config.user, "fixture");
        assert_eq!(config.port, 2222);
        assert_eq!(config.proxy_jump.as_deref(), Some("gateway"));
        assert!(
            config
                .identity_files
                .iter()
                .any(|path| path.ends_with(".ssh/eggie-fixture"))
        );
        let mut target = RemoteTarget::new("eggie-fixture".into());
        target.user = Some("override".into());
        target.port = Some(2200);
        let config = resolve_using_config(target, Some(&path)).unwrap();
        assert_eq!(config.user, "override");
        assert_eq!(config.port, 2200);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn quoted_includes_and_literal_hosts_are_discovered_without_exec() {
        let root = std::env::temp_dir().join(format!("ssh-cfg-{}", SessionId::new_v4()));
        std::fs::create_dir_all(root.join("parts")).unwrap();
        std::fs::write(root.join("config"),"Host direct *.example !excluded\nInclude \"parts/*.conf\"\nMatch exec \"must-not-run\"\n").unwrap();
        std::fs::write(root.join("parts/a.conf"), "Host=quoted\nInclude config\n").unwrap();
        let mut seen = HashSet::new();
        let mut hosts = HashSet::new();
        visit(
            &root.join("config"),
            &root,
            &root,
            &mut seen,
            &mut hosts,
            &mut 10000,
        )
        .unwrap();
        assert_eq!(hosts, HashSet::from(["direct".into(), "quoted".into()]));
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(
            diagnose("WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!"),
            Some(eggie_protocol::SshFailureKind::HostKey)
        );
        assert_eq!(
            diagnose("Permission denied (publickey)."),
            Some(eggie_protocol::SshFailureKind::Authentication)
        );
        assert_eq!(diagnose("application error"), None);
    }
}
