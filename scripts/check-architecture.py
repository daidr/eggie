#!/usr/bin/env python3
"""Check production dependency boundaries without compiling the workspace."""
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=ROOT
))
packages = {package["name"]: package for package in metadata["packages"]}
graph = {
    name: {dependency["name"] for dependency in package["dependencies"]
           if dependency["kind"] != "dev"}
    for name, package in packages.items()
}
# These lower layers have deliberately small, closed sets of direct dependencies.
allowed = {
    "eggie-domain": {"serde", "uuid", "bitflags"},
    "eggie-config": {"serde", "serde_json"},
    "eggie-storage": {"serde", "serde_json"},
    "eggie-themes": {"eggie-domain"},
    "eggie-ui-kit": {"gpui", "anyhow"},
    "eggie-build-info": set(),
}
# Views consume the domain cell contract, never the parser or a live daemon/client.
forbidden_direct = {
    "eggie-terminal-view": {"alacritty_terminal", "eggie-daemon", "eggie-ipc", "eggie-lua", "eggie-ui", "eggie-storage"},
    "eggie-ui": {"eggie-daemon", "eggie-lua", "mlua", "alacritty_terminal"},
    "eggie-daemon": {"eggie-ui", "eggie-ui-kit", "eggie-terminal-view", "gpui"},
}
forbidden_transitive = {
    "eggie-protocol": {"eggie-lua", "mlua", "gpui", "alacritty_terminal"},
    "eggie-ipc": {"eggie-daemon", "eggie-lua", "mlua", "gpui", "alacritty_terminal"},
    "eggie-update": {"eggie-daemon", "eggie-lua", "mlua", "gpui", "alacritty_terminal"},
    "eggie-updater": {"eggie-daemon", "eggie-lua", "mlua", "gpui", "alacritty_terminal"},
}

def reachable(name):
    found, pending = set(), [name]
    while pending:
        for dependency in graph.get(pending.pop(), ()):
            if dependency not in found:
                found.add(dependency)
                pending.append(dependency)
    return found

errors = []
for name, permitted in allowed.items():
    unexpected = graph[name] - permitted
    if unexpected:
        errors.append(f"{name}: unexpected dependencies {sorted(unexpected)}")
for name, forbidden in forbidden_direct.items():
    conflicts = graph[name] & forbidden
    if conflicts:
        errors.append(f"{name}: forbidden direct dependencies {sorted(conflicts)}")
for name, forbidden in forbidden_transitive.items():
    conflicts = reachable(name) & forbidden
    if conflicts:
        errors.append(f"{name}: forbidden reachable dependencies {sorted(conflicts)}")
if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
print(f"Architecture boundaries passed ({len(packages)} workspace crates).")
