#!/usr/bin/env python3
"""Verify that every API arrow in a .drawio diagram names a real proto symbol.

The convention is in docs/diagrams/README.md: an edge that represents an API carries
`proto` (file path) and `rpc` (either `Service.Method` or a message name). A diagram that
drifts from the interfaces is worse than no diagram, so this runs in CI.

Deliberately a small regex parser rather than a protobuf dependency: it needs to answer
"does this symbol exist", not to understand the schema, and the check must run with no
toolchain beyond Python.

Usage:  python3 tools/check_diagram.py [diagram.drawio ...]
Exit:   0 clean, 1 problems found.
"""

from __future__ import annotations

import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

_SERVICE_RE = re.compile(r"^\s*service\s+(\w+)\s*\{", re.M)
_RPC_RE = re.compile(r"^\s*rpc\s+(\w+)\s*\(", re.M)
_MESSAGE_RE = re.compile(r"^\s*message\s+(\w+)\s*\{", re.M)
_ENUM_RE = re.compile(r"^\s*enum\s+(\w+)\s*\{", re.M)


def symbols(path: Path) -> tuple[set[str], set[str]]:
    """Return (qualified rpc names, top-level type names) declared in a .proto file.

    Nesting is flattened: a message declared inside another is reported under its own bare
    name. That is looser than protobuf scoping, but the point is to catch typos and stale
    references, and a looser match never produces a false alarm.
    """
    text = path.read_text(encoding="utf-8")
    rpcs: set[str] = set()
    # Walk services in order so each rpc is attributed to the service it sits inside.
    bounds = [(m.group(1), m.start()) for m in _SERVICE_RE.finditer(text)]
    for i, (service, start) in enumerate(bounds):
        end = bounds[i + 1][1] if i + 1 < len(bounds) else len(text)
        for rpc in _RPC_RE.finditer(text, start, end):
            rpcs.add(f"{service}.{rpc.group(1)}")
    types = {m.group(1) for m in _MESSAGE_RE.finditer(text)}
    types |= {m.group(1) for m in _ENUM_RE.finditer(text)}
    return rpcs, types


def check(diagram: Path) -> list[str]:
    problems: list[str] = []
    root = ET.parse(diagram).getroot()

    edges = 0
    annotated = 0
    for page in root.findall("diagram"):
        page_name = page.get("name", "?")
        for obj in page.iter("object"):
            cell = obj.find("mxCell")
            if cell is None or cell.get("edge") != "1":
                continue
            edges += 1
            proto = obj.get("proto")
            rpc = obj.get("rpc")
            kind = obj.get("kind")
            label = obj.get("label", "")[:48]
            where = f"{diagram.name} [{page_name}] edge {obj.get('id')} {label!r}"

            if kind in {"internal", "event"} and not proto:
                continue  # data-flow hint, not an API contract
            if not proto and not rpc:
                problems.append(f"{where}: no proto/rpc and kind is {kind!r}; "
                                "set kind=internal if it is only a data-flow hint")
                continue
            annotated += 1
            if not proto:
                problems.append(f"{where}: has rpc={rpc!r} but no proto file")
                continue
            path = REPO / proto
            if not path.is_file():
                problems.append(f"{where}: proto file not found: {proto}")
                continue
            if not rpc:
                problems.append(f"{where}: has proto but no rpc")
                continue
            rpcs, types = symbols(path)
            bare = rpc.split(".")[-1]
            if rpc not in rpcs and rpc not in types and bare not in types:
                problems.append(
                    f"{where}: {rpc!r} not found in {proto}. "
                    f"Known rpcs: {sorted(rpcs)}"
                )

    # A box without a component pointer is a diagram that cannot be traced to code.
    for page in root.findall("diagram"):
        for obj in page.iter("object"):
            cell = obj.find("mxCell")
            if cell is None or cell.get("vertex") != "1":
                continue
            if not obj.get("component"):
                problems.append(
                    f"{diagram.name}: box {obj.get('label')!r} has no `component` property"
                )

    print(f"{diagram.name}: {edges} edges, {annotated} API-annotated, "
          f"{len(problems)} problem(s)")
    return problems


def main(argv: list[str]) -> int:
    targets = [Path(a) for a in argv[1:]] or sorted(
        (REPO / "docs" / "diagrams").glob("*.drawio")
    )
    if not targets:
        print("no .drawio files found")
        return 0
    problems: list[str] = []
    for t in targets:
        problems.extend(check(t))
    for p in problems:
        print(f"  FAIL {p}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
