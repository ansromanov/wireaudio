#!/usr/bin/env python3
"""Enforces a maximum line count (800 lines) on modified Rust source files."""

import pathlib
import sys

MAX_LINES = 800


def main() -> int:
    failed = False
    for raw in sys.stdin:
        rel = raw.strip()
        if not rel.endswith(".rs"):
            continue
        path = pathlib.Path(rel)
        if not path.is_file():
            continue
        lines = sum(1 for _ in path.open("r", encoding="utf-8", errors="ignore"))
        if lines > MAX_LINES:
            print(f"::error file={rel}::{rel} has {lines} lines (limit {MAX_LINES})")
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
