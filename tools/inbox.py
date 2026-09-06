#!/usr/bin/env python3
"""Find the user's in-file instructions to Claude, and commits Claude has not seen.

The user leaves instructions inside the files themselves, on a line beginning with their name
and a colon. Each is acted on and then deleted: a marker still present means the work is not
done, so there is no notion of an "answered" marker.

Also fetches from origin and reports commits not reachable from HEAD, since the user's own
edits arrive that way.

Scans two places, because an instruction can arrive either way:

  - the working tree, for instructions already merged;
  - every remote ref holding commits not reachable from HEAD, so an instruction pushed to the
    user's own branch is seen *before* it is merged. Without this, a marker on their branch
    stays invisible until someone thinks to merge, which defeats the point.

Usage:
    python3 tools/inbox.py                      # fetch, then report
    python3 tools/inbox.py --no-fetch           # report only, offline
    python3 tools/inbox.py --local              # working tree only, skip remote refs
    python3 tools/inbox.py --resolve FILE:LINE  # delete a marker block after acting on it

Exit codes: 0 nothing pending, 1 something pending. Non-zero is informational, not a failure;
it exists so a hook or a loop can branch on it.

Matching is anchored: the marker must be the first thing on the line after any leading
whitespace and comment syntax. Without that anchor this file and CLAUDE.md match themselves,
which the first version did.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Split so that this line does not itself match. Anchoring makes that unnecessary, but a
# grep-based search by a human should not turn up the tool's own source either.
MARKER = "Issao" + ":"

# Files that are not worth scanning. Binary and generated content only.
SKIP_SUFFIXES = {
    ".png", ".jpg", ".jpeg", ".gif", ".pdf", ".ico", ".woff", ".woff2", ".ttf",
    ".zip", ".gz", ".tar", ".bin", ".parquet", ".lock",
}
SKIP_DIRS = {".git", "target", "node_modules", "dist", ".venv", "out", "results"}

# Leading comment syntax to strip so the message text reads cleanly.
# `\*+(?=\s)` matches a doc-comment continuation like " * text" but NOT markdown bold
# "**text**", which an earlier version stripped, making prose that quoted an instruction look
# like a new instruction.
_COMMENT_PREFIX = re.compile(r"^\s*(?://+|\#+|/\*+|\*+/|\*+(?=\s)|<!--|--|;+|%+)?\s*")
_COMMENT_SUFFIX = re.compile(r"\s*(?:-->|\*/)\s*$")


def git(*args: str) -> str:
    out = subprocess.run(
        ["git", *args], cwd=REPO, capture_output=True, text=True, timeout=180
    )
    return out.stdout.strip()


def strip_comment(line: str) -> str:
    return _COMMENT_SUFFIX.sub("", _COMMENT_PREFIX.sub("", line)).strip()


def tracked_files() -> list[Path]:
    listing = git("ls-files")
    files = []
    for rel in listing.splitlines():
        p = REPO / rel
        if not p.is_file():
            continue
        if p.suffix.lower() in SKIP_SUFFIXES:
            continue
        if any(part in SKIP_DIRS for part in p.parts):
            continue
        files.append(p)
    return files


class Message:
    def __init__(self, path: Path, line_no: int, text: str, end_line: int) -> None:
        self.path = path
        self.line_no = line_no
        self.text = text
        #: Exclusive end of the marker block, for --resolve.
        self.end_line = end_line

    def render(self) -> str:
        rel = self.path.relative_to(REPO)
        span = "" if self.end_line == self.line_no else f"-{self.end_line}"
        return f"{rel}:{self.line_no}{span}\n    {self.text}"


def marker_at(line: str) -> str | None:
    """Return the message text if `line` *begins* a marker, else None.

    Anchored deliberately. A mid-line mention, or one inside backticks, is prose about the
    convention rather than an instruction, and must not match.
    """
    body = strip_comment(line)
    if not body.startswith(MARKER):
        return None
    return body[len(MARKER):].strip()


def scan(path: Path) -> list[Message]:
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    if MARKER not in "\n".join(lines):
        return []

    found: list[Message] = []
    i = 0
    while i < len(lines):
        head = marker_at(lines[i])
        if head is None:
            i += 1
            continue

        start = i
        parts = [head]

        # Continuation: following non-empty lines that do not start a new marker. A blank
        # line or a new marker ends the block.
        j = i + 1
        while j < len(lines):
            if marker_at(lines[j]) is not None:
                break
            body = strip_comment(lines[j])
            if not body:
                break
            parts.append(body)
            j += 1

        text = " ".join(p for p in parts if p)
        found.append(Message(path, start + 1, text or "(empty)", end_line=j))
        i = max(j, i + 1)

    return found


def resolve(spec: str) -> int:
    """Delete the marker block at FILE:LINE. Used once the repo reflects the instruction."""
    if ":" not in spec:
        print(f"--resolve needs FILE:LINE, got {spec!r}")
        return 2
    rel, _, line_s = spec.rpartition(":")
    path = (REPO / rel).resolve()
    try:
        want = int(line_s)
    except ValueError:
        print(f"--resolve needs an integer line, got {line_s!r}")
        return 2
    if not path.is_file():
        print(f"no such file: {rel}")
        return 2

    msgs = [m for m in scan(path) if m.line_no == want]
    if not msgs:
        print(f"no marker begins at {rel}:{want}")
        return 2
    m = msgs[0]

    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    removed = lines[m.line_no - 1 : m.end_line]
    del lines[m.line_no - 1 : m.end_line]
    path.write_text("".join(lines), encoding="utf-8")
    print(f"removed {len(removed)} line(s) from {rel}:{want}")
    for r in removed:
        print(f"  - {r.rstrip()}")
    print("Quote this instruction in the commit message so it survives in history.")
    return 0


def unmerged_refs() -> list[str]:
    """Remote refs that hold at least one commit not reachable from HEAD."""
    refs = git("for-each-ref", "--format=%(refname:short)", "refs/remotes").splitlines()
    out = []
    for ref in refs:
        ref = ref.strip()
        if not ref or ref.endswith("/HEAD"):
            continue
        if git("rev-list", "--count", "--max-count=1", f"{ref}", "--not", "HEAD").strip() not in ("", "0"):
            out.append(ref)
    return out


def scan_ref(ref: str) -> list[tuple[str, int, str]]:
    """Find markers in a remote ref without checking it out.

    Returns (path, line, text). `git grep` on a ref reads the object store directly, so this
    costs nothing and touches no files.
    """
    # Anchored the same way as marker_at: start of line, after optional comment syntax.
    pattern = r"^[[:space:]]*([/#*;%]|<!--|--)*[[:space:]]*" + MARKER
    raw = git("grep", "-n", "-I", "-E", pattern, ref, "--")
    found = []
    for line in raw.splitlines():
        # Format: <ref>:<path>:<lineno>:<content>
        rest = line[len(ref) + 1:] if line.startswith(ref + ":") else line
        parts = rest.split(":", 2)
        if len(parts) < 3:
            continue
        path, lineno, content = parts
        try:
            n = int(lineno)
        except ValueError:
            continue
        text = marker_at(content)
        if text is None:
            continue
        found.append((path, n, text or "(empty)"))
    return found


def new_commits(no_fetch: bool) -> list[str]:
    if not no_fetch:
        git("fetch", "--all", "--prune", "--quiet")
    # Everything reachable from any ref but not from HEAD.
    listing = git("log", "--oneline", "--all", "--not", "HEAD", "--max-count=40")
    return [ln for ln in listing.splitlines() if ln.strip()]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--no-fetch", action="store_true", help="skip git fetch")
    ap.add_argument("--local", action="store_true",
                    help="scan the working tree only, skipping unmerged remote refs")
    ap.add_argument("--resolve", metavar="FILE:LINE",
                    help="delete the marker block there, after acting on it")
    args = ap.parse_args()

    if args.resolve:
        return resolve(args.resolve)

    commits = new_commits(args.no_fetch)
    messages: list[Message] = []
    for path in tracked_files():
        messages.extend(scan(path))

    print("=" * 72)
    if commits:
        print(f"COMMITS not reachable from HEAD ({len(commits)}):")
        for c in commits:
            print(f"  {c}")
        print("  -> merge or rebase, then re-scan: new commits may carry new markers")
    else:
        print("COMMITS: none pending; HEAD has everything from origin")

    remote_hits: list[tuple[str, str, int, str]] = []
    if not args.local:
        for ref in unmerged_refs():
            for path, line, text in scan_ref(ref):
                remote_hits.append((ref, path, line, text))

    print("-" * 72)
    if messages:
        print(f"PENDING INSTRUCTIONS in the working tree ({len(messages)}):")
        for m in messages:
            print(f"  {m.render()}")
        print()
        print("  -> act on each one, fold it into the repo, then:")
        print("     python3 tools/inbox.py --resolve <file>:<line>")
        print("     and quote the instruction in the commit message.")
    else:
        print("PENDING INSTRUCTIONS in the working tree: none")

    if remote_hits:
        print()
        print(f"INSTRUCTIONS IN UNMERGED COMMITS ({len(remote_hits)}):")
        for ref, path, line, text in remote_hits:
            print(f"  {ref}:{path}:{line}\n    {text}")
        print()
        print("  -> merge that ref first, then act on them in the working tree.")
    print("=" * 72)

    return 1 if (commits or messages or remote_hits) else 0


if __name__ == "__main__":
    sys.exit(main())
