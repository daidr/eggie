#!/usr/bin/env python3
"""Capture macOS physical footprint and allocation categories for an explicit PID."""

import argparse
import ctypes
import datetime
import json
import pathlib
import re
import subprocess
import sys
import statistics
import time


def sample_footprint(pid, duration, interval, output):
    # rusage_info_v4 from the macOS SDK; libproc reads counters without scanning GPU VM regions.
    fields = "user_time system_time pkg_idle_wkups interrupt_wkups pageins wired_size resident_size phys_footprint proc_start_abstime proc_exit_abstime child_user_time child_system_time child_pkg_idle_wkups child_interrupt_wkups child_pageins child_elapsed_abstime diskio_bytesread diskio_byteswritten cpu_time_qos_default cpu_time_qos_maintenance cpu_time_qos_background cpu_time_qos_utility cpu_time_qos_legacy cpu_time_qos_user_initiated cpu_time_qos_user_interactive billed_system_time serviced_system_time logical_writes lifetime_max_phys_footprint instructions cycles billed_energy serviced_energy interval_max_phys_footprint runnable_time".split()

    class RUsage(ctypes.Structure):
        _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [
            (name, ctypes.c_uint64) for name in fields
        ]

    libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    libproc.proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
    libproc.proc_pid_rusage.restype = ctypes.c_int
    usage = RUsage()
    samples = []
    start = time.monotonic()
    while True:
        if libproc.proc_pid_rusage(pid, 4, ctypes.byref(usage)):
            raise OSError(ctypes.get_errno(), "proc_pid_rusage failed")
        elapsed = time.monotonic() - start
        samples.append({
            "elapsed_seconds": elapsed,
            "physical_footprint_bytes": usage.phys_footprint,
            "lifetime_peak_bytes": usage.lifetime_max_phys_footprint,
        })
        if elapsed >= duration:
            break
        time.sleep(min(interval, duration - elapsed))
    (output / "samples.json").write_text(json.dumps(samples, indent=2) + "\n")
    values = sorted(sample["physical_footprint_bytes"] for sample in samples)
    return {
        "sample_count": len(values),
        "duration_seconds": samples[-1]["elapsed_seconds"],
        "mean_footprint_bytes": round(statistics.mean(values)),
        "median_footprint_bytes": round(statistics.median(values)),
        "p95_footprint_bytes": values[min(len(values) - 1, int(len(values) * 0.95))],
        "min_footprint_bytes": values[0],
        "max_footprint_bytes": values[-1],
        "lifetime_peak_bytes": max(sample["lifetime_peak_bytes"] for sample in samples),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", type=int)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--heap", action="store_true", help="Also collect the slower heap census")
    parser.add_argument("--sample-seconds", type=float, default=0,
                        help="Use only lightweight libproc counters for this duration, avoiding VM scan perturbation")
    parser.add_argument("--interval", type=float, default=0.05, help="Sampling interval in seconds")
    args = parser.parse_args()
    if sys.platform != "darwin" or args.pid <= 0:
        parser.error("This profiler needs macOS and a positive process ID")
    if args.sample_seconds < 0 or args.interval <= 0 or (args.sample_seconds and args.heap):
        parser.error("Sampling needs a positive duration/interval and cannot include a heap scan")
    args.output.mkdir(parents=True, exist_ok=True)
    report = {
        "pid": args.pid,
        "captured_at": datetime.datetime.now().astimezone().isoformat(),
        "tools": {},
    }
    if args.sample_seconds:
        report["sampling"] = sample_footprint(args.pid, args.sample_seconds, args.interval, args.output)
        (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))
        return 0
    commands = {
        "vmmap-summary": ["vmmap", "-summary", str(args.pid)],
        "vmmap": ["vmmap", str(args.pid)],
        "footprint": ["footprint", "-p", str(args.pid)],
    }
    if args.heap:
        commands["heap"] = ["heap", "-s", str(args.pid)]
    failed = False
    for name, command in commands.items():
        try:
            result = subprocess.run(command, capture_output=True, text=True, timeout=30)
        except (OSError, subprocess.TimeoutExpired) as error:
            report["tools"][name] = {"error": str(error)}
            failed = True
            continue
        path = args.output / f"{name}.txt"
        path.write_text(result.stdout + result.stderr)
        report["tools"][name] = {"exit_code": result.returncode, "file": str(path.resolve())}
        failed |= result.returncode != 0
        if name == "vmmap-summary":
            for label, field in [
                (r"Physical footprint:", "physical_footprint_bytes"),
                (r"Physical footprint \(peak\):", "peak_footprint_bytes"),
            ]:
                match = re.search(label + r"\s+([\d.]+)([KMGT])", result.stdout)
                if match:
                    multiplier = 1024 ** ("KMGT".index(match[2]) + 1)
                    report[field] = round(float(match[1]) * multiplier)
    (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
