#!/usr/bin/env bash
set -euo pipefail

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

python3 - <<'PY'
import json
import subprocess
import sys

print("Checking layered architecture dependencies...", flush=True)

# Resolve once, including optional and target-specific dependencies. Cargo
# failures must fail this check, not masquerade as an absent dependency.
metadata = json.loads(subprocess.check_output([
    "cargo", "metadata", "--format-version", "1", "--all-features",
], text=True))
packages = {package["id"]: package["name"] for package in metadata["packages"]}
members = {packages[member]: member for member in metadata["workspace_members"]}
layers = [
    "helix-stdx nucleotide-types nucleotide-process",
    "helix-view nucleotide-logging nucleotide-workspace nucleotide-appearance nucleotide-terminal",
    "nucleotide-events nucleotide-env nucleotide-lsp-proxy",
    "nucleotide-core nucleotide-project nucleotide-remote nucleotide-vcs",
    "nucleotide-editor nucleotide-lsp",
    "nucleotide-ui",
    "nucleotide-terminal-view",
    "nucleotide-terminal-panel",
    "nucleotide",
]
layer_by_name = {
    name: layer for layer, names in enumerate(layers) for name in names.split()
}
errors = []
for name in sorted(members.keys() - layer_by_name.keys()):
    errors.append(f"Workspace crate {name} has no assigned layer")
for name in sorted(layer_by_name.keys() - members.keys()):
    errors.append(f"Layer entry {name} is not a workspace crate")

# Only normal dependencies define runtime layering; build/dev dependencies
# remain outside this policy. Walk without a depth limit.
graph = {
    node["id"]: {
        dep["pkg"] for dep in node["deps"]
        if any(kind["kind"] is None for kind in dep["dep_kinds"])
    }
    for node in metadata["resolve"]["nodes"]
}
ui_free_crates = {
    "nucleotide-process", "nucleotide-logging", "nucleotide-workspace",
    "nucleotide-terminal", "nucleotide-events", "nucleotide-env",
    "nucleotide-lsp-proxy", "nucleotide-lsp", "nucleotide-project",
    "nucleotide-remote",
}
for name, member in sorted(members.items()):
    visited = set()
    pending = list(graph[member])
    while pending:
        dependency = pending.pop()
        if dependency in visited:
            continue
        visited.add(dependency)
        pending.extend(graph[dependency] - visited)
    dependencies = {packages[dependency] for dependency in visited}
    for dependency in sorted(dependencies & layer_by_name.keys()):
        if name in layer_by_name and layer_by_name[dependency] > layer_by_name[name]:
            errors.append(f"{name} depends on higher-layer {dependency}")
    if name in ui_free_crates:
        for dependency in sorted(dependencies & {"gpui", "nucleotide-ui"}):
            errors.append(f"{name} depends on UI crate {dependency}")

# Check the lightweight types configuration separately: the all-feature
# workspace intentionally enables its optional GPUI and Helix bridges.
types_tree = subprocess.check_output([
    "cargo", "tree", "-p", "nucleotide-types", "--no-default-features",
    "--edges", "normal", "--target", "all", "--prefix", "none",
], text=True)
heavy = {"gpui", "helix-core", "helix-view", "helix-term"}
for line in types_tree.splitlines():
    if line.split()[0] in heavy:
        errors.append(f"nucleotide-types has a heavy dependency without features: {line}")

for error in errors:
    print(f"ERROR: {error}")
if errors:
    print(f"Found {len(errors)} layering violations")
    sys.exit(1)
print(f"All layering checks passed ({len(members)} workspace crates)")
PY
