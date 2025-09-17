#!/usr/bin/env python3
"""
Lowercase the first character of the message string in tracing macros across the repo.

Targets macros from the `tracing` crate: trace!, debug!, info!, warn!, error!

For each macro invocation, it finds the first regular string literal "..." inside the
macro arguments and lowercases its first character if it's an ASCII uppercase letter.

Notes:
- This is a simple best-effort script; it doesn't fully parse Rust. It aims to be
  safe by making minimal edits.
- It skips build/output folders (target/, release/, doc/, package/, tmp/, .git/).
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path
from typing import Tuple, Optional


ROOT = Path(__file__).resolve().parents[1]
EXCLUDE_DIRS = {"target", "release", "doc", "package", "tmp", ".git"}
MACRO_PATTERN = re.compile(r"\b(trace|debug|info|warn|error)!\s*\(")


def find_matching_paren(text: str, open_idx: int) -> Optional[int]:
    """Given index of an opening parenthesis, find its matching closing index.

    Returns the index of the matching ')' or None if not found.
    """
    depth = 0
    i = open_idx
    in_str = False
    in_char = False
    escape = False

    while i < len(text):
        ch = text[i]

        if in_str:
            if escape:
                escape = False
            elif ch == "\\":
                escape = True
            elif ch == '"':
                in_str = False
        elif in_char:
            if escape:
                escape = False
            elif ch == "\\":
                escape = True
            elif ch == "'":
                in_char = False
        else:
            if ch == '"':
                in_str = True
            elif ch == "'":
                in_char = True
            elif ch == '(':
                depth += 1
            elif ch == ')':
                depth -= 1
                if depth == 0:
                    return i
        i += 1

    return None


STRING_LITERAL_PATTERN = re.compile(r'"((?:\\.|[^"\\])*)"', re.DOTALL)


def lowercase_first_char(s: str) -> str:
    if not s:
        return s
    first = s[0]
    # Only touch ASCII uppercase letters for safety
    if 'A' <= first <= 'Z':
        return first.lower() + s[1:]
    return s


def process_macro_args(args: str) -> Tuple[str, bool]:
    """Find the first normal string literal in macro args and lowercase its first char.

    Returns (new_args, changed)
    """
    m = STRING_LITERAL_PATTERN.search(args)
    if not m:
        return args, False

    start, end = m.span(1)  # content group indices
    content = m.group(1)

    new_content = lowercase_first_char(content)
    if new_content == content:
        return args, False

    # Rebuild args with replaced content
    new_args = args[:start] + new_content + args[end:]
    return new_args, True


def process_file(path: Path) -> Tuple[bool, int]:
    text = path.read_text(encoding="utf-8")
    changed = False
    changes_in_file = 0

    # Scan for macros; iterate left-to-right and reconstruct as we go
    i = 0
    out_parts = []
    while True:
        m = re.search(r"\b(trace|debug|info|warn|error)!\s*\(", text[i:])
        if not m:
            out_parts.append(text[i:])
            break
        macro_start = i + m.start()
        open_paren = i + m.end() - 1  # index of '('

        # Append up to macro start
        out_parts.append(text[i:macro_start])

        close_paren = find_matching_paren(text, open_paren)
        if close_paren is None:
            # Fallback: append the rest and stop to avoid corrupting file
            out_parts.append(text[macro_start:])
            i = len(text)
            break

    # Extract args inside parentheses
        args = text[open_paren + 1:close_paren]
        new_args, did_change = process_macro_args(args)

        if did_change:
            changed = True
            changes_in_file += 1
        # Reconstruct macro with possibly updated args
        out_parts.append(text[macro_start:open_paren + 1])
        out_parts.append(new_args)
        out_parts.append(')')

        i = close_paren + 1

    if changed:
        new_text = ''.join(out_parts)
        path.write_text(new_text, encoding="utf-8")

    return changed, changes_in_file


def should_skip_dir(dirpath: Path) -> bool:
    return any(part in EXCLUDE_DIRS for part in dirpath.parts)


def main() -> int:
    root = ROOT
    total_files = 0
    modified_files = 0
    total_changes = 0

    for dirpath, dirnames, filenames in os.walk(root):
        # Prune excluded directories in-place to avoid walking them
        dirnames[:] = [d for d in dirnames if d not in EXCLUDE_DIRS]
        if any(part in EXCLUDE_DIRS for part in Path(dirpath).parts):
            continue

        for filename in filenames:
            if not filename.endswith('.rs'):
                continue
            path = Path(dirpath) / filename
            total_files += 1
            try:
                changed, n = process_file(path)
            except UnicodeDecodeError:
                continue
            if changed:
                modified_files += 1
                total_changes += n

    print(f"Scanned {total_files} Rust files under {root}.")
    print(f"Modified {modified_files} files; updated {total_changes} tracing message(s).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
