#!/usr/bin/env python3
"""Prepare an independent official checkout with only a test harness and version stamping."""
import argparse
import json
import runpy
import subprocess
from pathlib import Path

COMMIT = "092d3acd6bec3e3a14bdc7e7a2810ab628ab759d"

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    destination = args.destination.resolve()
    if destination.exists() and any(destination.iterdir()):
        parser.error("destination must be empty; existing references are never overwritten")
    destination.mkdir(parents=True, exist_ok=True)
    archive = subprocess.Popen(["git", "archive", COMMIT], cwd=repo, stdout=subprocess.PIPE)
    subprocess.run(["tar", "-x", "-C", str(destination)], stdin=archive.stdout, check=True)
    archive.stdout.close()
    if archive.wait():
        raise RuntimeError("git archive failed")
    # Official tagged manifests and workspace lockfile versions need matching build metadata.
    runpy.run_path(str(repo / "deploy/cpa-runtime/prepare-release.py"))["prepare_release"](destination, "0.162.1")
    fixture = (repo / "codex-rs/core/src/native_model_blackbox_tests.rs").read_text()
    fixture = fixture.split("// BEGIN MODEL_ONLY_BOUNDARY_TESTS")[0]
    start, end = fixture.index("// BEGIN EXECUTOR"), fixture.index("// END EXECUTOR")
    executor = (repo / "deploy/native-runtime/official-executor.rs").read_text()
    fixture = (fixture[:start] + executor + fixture[end + len("// END EXECUTOR"):]).replace("use std::sync::Mutex;\n", "")
    (destination / "codex-rs/core/src/session/native_model_blackbox_tests.rs").write_text(fixture)
    module = destination / "codex-rs/core/src/session/mod.rs"
    module.write_text(module.read_text() + "\n#[cfg(test)]\nmod native_model_blackbox_tests;\n")
    (destination / "native-reference.json").write_text(json.dumps({"commit": COMMIT, "version": "0.162.1", "changes": ["workspace build-version metadata", "test-only fixture module and executor"]}, indent=2) + "\n")
    print(destination)

if __name__ == "__main__":
    main()
