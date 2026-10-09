#!/usr/bin/env python3
"""Run actionlint, validating its one known concurrency.queue schema gap.

GitHub supports queue:max (2026-05-07), but actionlint 1.7.12 does not:
https://github.com/rhysd/actionlint/issues/657
https://github.blog/changelog/2026-05-07-github-actions-concurrency-groups-now-allow-larger-queues/

Do not use a broad -ignore: invalid values, cancelling queues and unrelated
syntax errors must still fail. Only the exact diagnostic at a validated
top-level queue property is compatible. Once upstream supports it, this
wrapper continues running all checks without needing an ignored diagnostic.
"""
import json
from pathlib import Path
import re
import subprocess
import sys

UNSUPPORTED = (
    'unexpected key "queue" for "concurrency" section. '
    'expected one of "cancel-in-progress", "group"'
)


def supported_queue_line(text):
    """Recognize only our explicit block style; never reinterpret YAML."""
    properties = {}
    active = False
    current_key = None
    continued_scalars = []
    for number, line in enumerate(text.splitlines(), 1):
        content = line.split("#", 1)[0].rstrip()
        if not content.strip():
            continue
        if content == "concurrency:":
            if active or properties:
                raise ValueError("duplicate top-level concurrency block")
            active = True
            continue
        if not active:
            continue
        if not content.startswith(" "):
            active = False
            continue
        match = re.fullmatch(r"  ([a-z-]+):\s*(.*)", content)
        if not match:
            if current_key in {"queue", "cancel-in-progress"}:
                continued_scalars.append(current_key)
            continue
        key, value = match.groups()
        current_key = key
        if key in properties:
            raise ValueError(f"duplicate concurrency property: {key}")
        properties[key] = (value, number)
    if "queue" not in properties:
        return None
    if continued_scalars:
        raise ValueError(f"{continued_scalars[0]} must be a single literal scalar")
    if set(properties) != {"group", "queue", "cancel-in-progress"}:
        raise ValueError("queued releases need explicit group, queue and cancel-in-progress")
    if properties["queue"][0] != "max" or properties["cancel-in-progress"][0] != "false":
        raise ValueError("queued releases require literal queue: max and cancel-in-progress: false")
    if not properties["group"][0]:
        raise ValueError("queued releases need a nonempty group")
    return properties["queue"][1]


def decode_diagnostics(output):
    decoder = json.JSONDecoder()
    diagnostics = []
    while output.strip():
        entries, end = decoder.raw_decode(output.lstrip())
        if entries is not None:
            if not isinstance(entries, list) or not all(isinstance(e, dict) for e in entries):
                raise ValueError("unexpected actionlint diagnostic format")
            diagnostics.extend(entries)
        output = output.lstrip()[end:]
    return diagnostics


def main(argv):
    paths = [Path(p) for p in argv] if argv else sorted(Path(".github/workflows").glob("*.y*ml"))
    if not paths:
        raise ValueError("no workflow files found")
    allowed = {str(p.resolve()): supported_queue_line(p.read_text()) for p in paths}
    result = subprocess.run(
        ["actionlint", "-no-color", "-format", "{{json .}}", *map(str, paths)],
        text=True, capture_output=True,
    )
    if result.stderr:
        print(result.stderr, end="", file=sys.stderr)
    errors = decode_diagnostics(result.stdout)
    rejected = []
    for error in errors:
        expected_line = allowed.get(str(Path(error.get("filepath", "")).resolve()))
        compatible = (
            error.get("kind") == "syntax-check"
            and error.get("message") == UNSUPPORTED
            and error.get("column") == 3
            and isinstance(expected_line, int)
            and expected_line == error.get("line")
        )
        if not compatible:
            rejected.append(error)
            print(f'{error.get("filepath")}:{error.get("line")}:{error.get("column")}: '
                  f'{error.get("message")} [{error.get("kind")}]', file=sys.stderr)
    if rejected or result.returncode not in (0, 1) or (result.returncode and not errors):
        return 1
    # Tool failures must not disappear behind an otherwise compatible diagnostic.
    if result.stderr.strip():
        return 1
    print(f"Workflow lint passed for {len(paths)} files; concurrency queues validated.")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except (OSError, ValueError) as error:
        print(f"Workflow lint failed: {error}", file=sys.stderr)
        sys.exit(1)
