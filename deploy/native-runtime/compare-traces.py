#!/usr/bin/env python3
"""Compare fixed-official and adapter wire/state traces with explicit normalization.

Only mapped session/window UUIDs, generated native item UUIDs, creation times and
WebSocket request timestamps/traces are normalized. Field presence, identity
relationships, retry ordering/counts, payloads and native results remain compared.
"""
import argparse
import json
import re
from pathlib import Path

UUID = re.compile(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$")
ITEM = re.compile(r"(msg|at)_([0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})$")

def load(path):
    records = {}
    for line in Path(path).read_text().splitlines():
        record = json.loads(line)
        if record["case"] in records:
            raise ValueError(f"duplicate case: {record['case']}")
        records[record["case"]] = record["trace"]
    return records

def normalize(trace):
    ids = {}
    item_ids = {}
    for request in trace["requests"]:
        metadata = request.get("client_metadata", {})
        for key, label in [("session_id", "session"), ("thread_id", "thread"), ("x-codex-window-id", "window")]:
            value = metadata.get(key)
            if isinstance(value, str) and UUID.fullmatch(value):
                ids.setdefault(value, f"<{label}-{len(ids)}>")
    def visit(value, key=None):
        if isinstance(value, dict):
            return {k: visit(v, k) for k, v in value.items()}
        if isinstance(value, list):
            return [visit(v) for v in value]
        if key == "x-codex-turn-metadata" and isinstance(value, str):
            return visit(json.loads(value))
        if key in ("create_time", "x-codex-ws-stream-request-start-ms"):
            return "<native-time>"
        if key == "ws_request_header_traceparent":
            return "<native-trace>"
        if isinstance(value, str):
            if value in ids:
                return ids[value]
            if key == "id" and (match := ITEM.fullmatch(value)):
                return item_ids.setdefault(value, f"<{match[1]}-{len(item_ids)}>")
        return value
    return visit(trace)

def differences(left, right, path="$"):
    if type(left) is not type(right):
        return [(path, left, right)]
    if isinstance(left, dict):
        found = []
        for key in sorted(left.keys() | right.keys()):
            if key not in left or key not in right:
                found.append((f"{path}.{key}", left.get(key, "<absent>"), right.get(key, "<absent>")))
            else:
                found.extend(differences(left[key], right[key], f"{path}.{key}"))
        return found
    if isinstance(left, list):
        if len(left) != len(right):
            return [(path + ".length", len(left), len(right))]
        return [d for i, (a, b) in enumerate(zip(left, right)) for d in differences(a, b, f"{path}[{i}]")]
    return [] if left == right else [(path, left, right)]

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("official")
    parser.add_argument("adapter")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    official, adapter = load(args.official), load(args.adapter)
    if official.keys() != adapter.keys():
        raise ValueError("scenario sets differ")
    results = []
    for name, reference in official.items():
        diffs = differences(normalize(reference), normalize(adapter[name]))
        results.append({"case": name, "passed": not diffs, "differences": diffs})
        print(f"{'PASS' if not diffs else 'FAIL'} {name}")
        for path, before, after in diffs[:6]:
            print(f"  {path}: official={before!r}; adapter={after!r}")
    report = {"officialCommit": "092d3acd6bec3e3a14bdc7e7a2810ab628ab759d", "results": results}
    Path(args.output).write_text(json.dumps(report, indent=2) + "\n")
    raise SystemExit(0 if all(r["passed"] for r in results) else 1)

if __name__ == "__main__":
    main()
