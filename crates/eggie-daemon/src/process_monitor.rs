//! Descendant-process discovery and listening-port probing for session inspection.
//!
//! Pure, stateless helpers driven by the ~1s inspection poll (not a per-frame path). Extracted
//! verbatim from `lib.rs`; `use super::*` re-exports keep the call sites and tests unchanged.

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use eggie_protocol::{ListeningPort, ProcessInfo};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

pub(crate) fn descendant_processes(root_pid: u32, system: &mut System) -> Vec<ProcessInfo> {
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        ProcessRefreshKind::new().with_cpu().with_memory(),
    );
    let all_processes = system
        .processes()
        .values()
        .map(|process| ProcessInfo {
            pid: process.pid().as_u32(),
            parent_pid: process.parent().map(|pid| pid.as_u32()),
            name: process.name().to_string_lossy().into_owned(),
            cpu_usage_tenths_percent: Some(cpu_usage_tenths_percent(process.cpu_usage())),
            memory_bytes: Some(process.memory()),
        })
        .collect::<Vec<_>>();
    filter_descendant_processes(root_pid, all_processes)
}

pub(crate) fn cpu_usage_tenths_percent(usage: f32) -> u32 {
    if usage.is_finite() && usage > 0. {
        (usage * 10.).round().min(u32::MAX as f32) as u32
    } else {
        0
    }
}

pub(crate) fn filter_descendant_processes(root_pid: u32, mut processes: Vec<ProcessInfo>) -> Vec<ProcessInfo> {
    processes.sort_by_key(|process| process.pid);
    let mut included = HashSet::from([root_pid]);
    let mut result = Vec::new();
    let mut pending = processes;
    loop {
        let mut changed = false;
        pending.retain(|process| {
            if process.pid == root_pid
                || process
                    .parent_pid
                    .is_some_and(|pid| included.contains(&pid))
            {
                included.insert(process.pid);
                result.push(process.clone());
                changed = true;
                false
            } else {
                true
            }
        });
        if !changed {
            break;
        }
    }
    result
}

pub(crate) fn listening_ports(processes: &[ProcessInfo]) -> Vec<ListeningPort> {
    if processes.is_empty() {
        return Vec::new();
    }
    let pids = processes
        .iter()
        .map(|process| process.pid.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let lsof = if Path::new("/usr/sbin/lsof").exists() {
        "/usr/sbin/lsof"
    } else {
        "lsof"
    };
    let Ok(output) = Command::new(lsof)
        .args([
            "-nP",
            "-a",
            "-p",
            &pids,
            "-iTCP",
            "-sTCP:LISTEN",
            "-iUDP",
            "-FpcPn",
        ])
        .output()
    else {
        return Vec::new();
    };
    parse_lsof_ports(&String::from_utf8_lossy(&output.stdout))
}

pub(crate) fn parse_lsof_ports(output: &str) -> Vec<ListeningPort> {
    let mut current_pid = None;
    let mut current_protocol = None;
    let mut ports = Vec::new();
    for field in output.lines().filter(|line| line.len() >= 2) {
        let (kind, value) = field.split_at(1);
        match kind {
            "p" => current_pid = value.parse::<u32>().ok(),
            "f" => current_protocol = None,
            "P" => current_protocol = Some(value.to_owned()),
            "n" => {
                let Some(pid) = current_pid else {
                    continue;
                };
                let Some(protocol) = current_protocol.clone() else {
                    continue;
                };
                let endpoint = value.split("->").next().unwrap_or(value);
                let Some((address, port)) = endpoint.rsplit_once(':') else {
                    continue;
                };
                let Ok(port) = port.parse::<u16>() else {
                    continue;
                };
                ports.push(ListeningPort {
                    pid,
                    protocol,
                    address: address.to_owned(),
                    port,
                });
            }
            _ => {}
        }
    }
    ports.sort_by(|left, right| {
        (&left.protocol, &left.address, left.port, left.pid).cmp(&(
            &right.protocol,
            &right.address,
            right.port,
            right.pid,
        ))
    });
    ports.dedup_by(|left, right| left == right);
    ports
}

/// Marker separating the `ps` block from the `lsof` block in the remote inspection probe.
const REMOTE_LSOF_MARKER: &str = "===EGGIE-LSOF===";

/// Parse the combined remote inspection probe into the session's processes and ports.
///
/// The probe prints `ROOT <sshd-session-pid>` (the session anchor, the parent of the shell running
/// the probe), `SELF <probe-shell-pid>` (the throwaway shell running the probe, plus the `ps` it
/// forks), a full `ps -eo` listing, then [`REMOTE_LSOF_MARKER`] and `lsof` output. Filtering is by
/// **process ancestry** from ROOT, not by tty: on Linux a dead session's background shells are
/// reparented to init (ppid 1) without releasing the controlling terminal, and pts numbers get
/// recycled, so a tty filter drags in zombies from other sessions. The probe's own subtree is
/// removed so it does not appear as a second top-level shell.
pub(crate) fn parse_remote_inspection(output: &str) -> (Vec<ProcessInfo>, Vec<ListeningPort>) {
    let empty = (Vec::new(), Vec::new());
    let mut root = None;
    let mut session_root = None;
    let mut self_pid = None;
    let mut ps_lines: Vec<&str> = Vec::new();
    let mut lsof = String::new();
    let mut in_lsof = false;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("ROOT ") {
            root = value.trim().parse::<u32>().ok();
        } else if let Some(value) = line.strip_prefix("SESSION ") {
            session_root = value.trim().parse::<u32>().ok();
        } else if let Some(value) = line.strip_prefix("SELF ") {
            self_pid = value.trim().parse::<u32>().ok();
        } else if line.trim() == REMOTE_LSOF_MARKER {
            in_lsof = true;
        } else if in_lsof {
            lsof.push_str(line);
            lsof.push('\n');
        } else {
            ps_lines.push(line);
        }
    }
    let Some(root) = session_root.or(root) else {
        return empty;
    };

    // parse_remote_ps skips the first line as a header; the probe's ps output already carries its
    // own header, so give it a throwaway one.
    let ps_input = std::iter::once("  PID  PPID  %CPU   RSS COMM")
        .chain(ps_lines.iter().copied())
        .collect::<Vec<_>>()
        .join("\n");
    let all = parse_remote_ps(&ps_input);

    // The probe shell and everything it spawned (the `ps` itself) must not show up as session
    // processes.
    let probe_pids: HashSet<u32> = match self_pid {
        Some(pid) => filter_descendant_processes(pid, all.clone())
            .into_iter()
            .map(|process| process.pid)
            .collect(),
        None => HashSet::new(),
    };
    let mut processes = filter_descendant_processes(root, all);
    processes.retain(|process| {
        (session_root.is_some() || process.pid != root) && !probe_pids.contains(&process.pid)
    });

    let pids: HashSet<u32> = processes.iter().map(|process| process.pid).collect();
    let ports = parse_lsof_ports(&lsof)
        .into_iter()
        .filter(|port| pids.contains(&port.pid))
        .collect();

    (processes, ports)
}

/// Parse the output of `ps -eo pid,ppid,pcpu,rss,comm` into process info for a remote session.
///
/// The header line is skipped, and any line that does not parse is ignored rather than failing the
/// whole inspection. CPU is converted from a percentage to tenths, and RSS from KiB to bytes, to
/// match the local process info.
pub(crate) fn parse_remote_ps(output: &str) -> Vec<ProcessInfo> {
    let mut processes = Vec::new();
    for line in output.lines().skip(1) {
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let parent_pid = fields.next().and_then(|value| value.parse::<u32>().ok());
        let cpu_usage_tenths_percent = fields
            .next()
            .and_then(|value| value.parse::<f32>().ok())
            .map(|value| (value * 10.).round() as u32);
        let memory_bytes = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .map(|value| value * 1024);
        // The command may contain spaces, so join the remaining fields.
        let name = fields.collect::<Vec<_>>().join(" ");
        if name.is_empty() {
            continue;
        }
        processes.push(ProcessInfo {
            pid,
            parent_pid,
            name,
            cpu_usage_tenths_percent,
            memory_bytes,
        });
    }
    processes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_ps_output_parses_into_process_info() {
        let output = "  PID  PPID  %CPU   RSS COMM\n\
                      1     0   1.3  1234 /sbin/launchd\n\
                    100     1   0.0   567 /usr/bin/sshd\n";
        let processes = parse_remote_ps(output);
        assert_eq!(processes.len(), 2, "both processes parse");
        assert_eq!(processes[0].pid, 1);
        assert_eq!(processes[0].parent_pid, Some(0));
        assert_eq!(processes[0].cpu_usage_tenths_percent, Some(13), "1.3% → 13 tenths");
        assert_eq!(processes[0].memory_bytes, Some(1234 * 1024), "KiB → bytes");
        assert_eq!(processes[0].name, "/sbin/launchd");
        assert_eq!(processes[1].pid, 100);
        assert_eq!(processes[1].parent_pid, Some(1));
    }

    #[test]
    fn remote_ps_skips_unparseable_lines() {
        let output = "  PID  PPID  %CPU   RSS COMM\n\
                    not-a-pid  0   1.0  1234 /bin/foo\n\
                    200     0   1.0  1234\n";
        let processes = parse_remote_ps(output);
        assert!(processes.is_empty(), "neither line parses: bad pid, then missing command");
    }

    #[test]
    fn remote_inspection_keeps_only_the_session_tree() {
        // Mirrors a real Linux host (observed on a shared dev box): the session's own zsh (709) sits
        // under the sshd session (ROOT 672), while earlier sessions left zsh shells reparented to
        // init (ppid 1) that still hold the recycled pts — they must NOT appear. The probe shell
        // (SELF 800) and the `ps` it forks are also excluded.
        let output = "ROOT 672\n\
SELF 800\n\
  PID  PPID  %CPU   RSS COMMAND\n\
  672   644   0.0  4000 sshd: user@pts/17\n\
  709   672   0.0  8568 zsh\n\
  716     1   0.0  3988 zsh\n\
  752     1   0.0  4820 zsh\n\
  800   672   0.0  3552 sh\n\
  801   800   0.0  4000 ps\n\
===EGGIE-LSOF===\n\
p709\n\
f3\n\
PIPv4\n\
n127.0.0.1:3000\n\
p716\n\
f5\n\
PIPv4\n\
n127.0.0.1:9999\n";
        let (processes, ports) = parse_remote_inspection(output);
        let pids: Vec<u32> = processes.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![709], "only the session zsh remains; orphans and the probe are dropped");
        // Port of the session zsh survives; the orphan's port (9999) is filtered out.
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, 3000);
        assert_eq!(ports[0].pid, 709);
    }

    #[test]
    fn remote_inspection_without_root_yields_empty() {
        // A host whose shell did not expand $PPID (or truncated output) degrades to empty rather than
        // treating pid 0 as the root.
        let output = "SELF 800\n  PID  PPID COMMAND\n  1     0 launchd\n===EGGIE-LSOF===\n";
        let (processes, ports) = parse_remote_inspection(output);
        assert!(processes.is_empty());
        assert!(ports.is_empty());
    }

    #[test]
    fn remote_inspection_tolerates_missing_lsof() {
        // `lsof ... || true` produces a marker with nothing after it; ps must still parse.
        let output = "ROOT 672\n\
SELF 800\n\
  PID  PPID  %CPU   RSS COMMAND\n\
  672   644   0.0  4000 sshd: user@pts/17\n\
  709   672   0.0  8568 zsh\n\
  800   672   0.0  3552 sh\n\
===EGGIE-LSOF===\n";
        let (processes, ports) = parse_remote_inspection(output);
        assert_eq!(processes.len(), 1, "the session zsh is still reported");
        assert_eq!(processes[0].pid, 709);
        assert!(ports.is_empty(), "no lsof output means no ports, not an error");
    }
}
