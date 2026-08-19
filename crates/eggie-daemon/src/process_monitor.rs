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
