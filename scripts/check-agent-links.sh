#!/usr/bin/env bash
# Checks that every relative Markdown link under agents/ and in the root *.md files resolves to a
# file or directory inside the repository, that a #fragment names a heading in its target, and that
# the root AGENTS.md and CLAUDE.md are symlinks to agents/AGENTS.md. Inline links (with a title in
# quotes or parentheses, an angle-bracket destination, or a destination on the next line) and
# reference definitions are read; code spans and fenced blocks (backtick or tilde) are not. A link
# into the gitignored agents/.processes/ or a pulled vendor pack is reported, because a fresh
# checkout lacks it. Needs python3. Exits 1 and prints each broken link.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 -I - "$root" <<'PY'
import os
import re
import sys

root = os.path.realpath(sys.argv[1])
problems = []

for name in ("AGENTS.md", "CLAUDE.md"):
    path = os.path.join(root, name)
    if not os.path.islink(path):
        problems.append(f"{name}: not a symlink")
    elif os.path.realpath(path) != os.path.join(root, "agents", "AGENTS.md"):
        problems.append(f"{name}: does not point to agents/AGENTS.md")


def local_only(rel_dir):
    """True for the gitignored run directory and for each pulled vendor pack."""
    return rel_dir == os.path.join("agents", ".processes") or os.path.dirname(rel_dir) == os.path.join(
        "agents", "skills", "vendor"
    )


def ignored(real):
    """True for a path inside the gitignored run directory or a pulled vendor pack."""
    parts = os.path.relpath(real, root).split(os.sep)
    if parts[:2] == ["agents", ".processes"]:
        return True
    return parts[:3] == ["agents", "skills", "vendor"] and len(parts) > 3 and os.path.isdir(os.path.join(root, *parts[:4]))


files = []
for entry in sorted(os.listdir(root)):
    full = os.path.join(root, entry)
    if entry.endswith(".md") and os.path.isfile(full) and not os.path.islink(full):
        files.append(full)
for base, dirs, names in os.walk(os.path.join(root, "agents")):
    dirs[:] = sorted(d for d in dirs if not local_only(os.path.relpath(os.path.join(base, d), root)))
    for n in sorted(names):
        if n.endswith(".md"):
            files.append(os.path.join(base, n))

fence_open = re.compile(r"^ {0,3}(`{3,}|~{3,})")


def prose(path):
    """The file's lines with fenced blocks blanked (keeping line numbers), and the headings found."""
    lines, headings, fence = [], [], None
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            match = fence_open.match(line)
            if fence is None and match:
                fence = match.group(1)
                lines.append("\n")
            elif fence is not None:
                if line.strip().startswith(fence[0] * len(fence)) and not line.strip().strip(fence[0]):
                    fence = None
                lines.append("\n")
            else:
                lines.append(line)
                heading = re.match(r"#{1,6}\s+(.*)", line)
                if heading:
                    headings.append(heading.group(1))
    return lines, headings


def slug(heading):
    text = re.sub(r"`", "", heading.strip().lower())
    text = re.sub(r"[^\w\- ]", "", text)
    return text.replace(" ", "-")


def anchors(headings):
    found, counts = set(), {}
    for heading in headings:
        base = slug(heading)
        n = counts.get(base, 0)
        counts[base] = n + 1
        found.add(base if n == 0 else f"{base}-{n}")
    return found


cache = {}


def headings_of(path):
    if path not in cache:
        cache[path] = anchors(prose(path)[1])
    return cache[path]


title = r"""(?:\s+(?:"[^"]*"|'[^']*'|\([^)]*\)))?"""
inline = re.compile(r"\]\(\s*(<[^>]+>|[^)\s]+)" + title + r"\s*\)")
definition = re.compile(r"^ {0,3}\[[^\]]+\]:\s*(<[^>]+>|\S+)", re.MULTILINE)
code_span = re.compile(r"`[^`]*`")
inside = root + os.sep

for path in files:
    rel = os.path.relpath(path, root)
    lines = prose(path)[0]
    text = "".join(lines)
    text = code_span.sub(lambda m: " " * len(m.group(0)), text)
    found = [(m.start(1), m.group(1)) for m in inline.finditer(text)]
    found += [(m.start(1), m.group(1)) for m in definition.finditer(text)]
    for offset, target in sorted(found):
        number = text.count("\n", 0, offset) + 1
        target = target.strip("<>")
        if re.match(r"[a-z][a-z0-9+.-]*:", target):
            continue
        file_part, _, fragment = target.partition("#")
        resolved = path if not file_part else os.path.normpath(os.path.join(os.path.dirname(path), file_part))
        real = os.path.realpath(resolved)
        if not os.path.exists(resolved):
            problems.append(f"{rel}:{number}: {target} does not resolve")
        elif not (real + os.sep).startswith(inside):
            problems.append(f"{rel}:{number}: {target} leaves the repository")
        elif ignored(real):
            problems.append(f"{rel}:{number}: {target} is local only (gitignored)")
        elif fragment and os.path.isfile(resolved) and resolved.endswith(".md"):
            if fragment not in headings_of(resolved):
                problems.append(f"{rel}:{number}: {target} names no heading")

for problem in problems:
    print(problem)
sys.exit(1 if problems else 0)
PY
