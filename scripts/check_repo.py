#!/usr/bin/env python3
"""Run dependency-free repository consistency checks for CI."""

from __future__ import annotations

import csv
import json
import re
import sys
from pathlib import Path
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parents[1]
TEXT_SUFFIXES = {".csv", ".md", ".py", ".rs", ".toml", ".wgsl", ".yaml", ".yml"}
MARKDOWN_LINK = re.compile(r"!?\[[^\]]*\]\((<[^>]+>|[^\s)]+)(?:\s+[^)]*)?\)")
PERSONAL_PATH = re.compile(r"(?:/home/|[A-Za-z]:[\\/](?:Users|Documents and Settings)[\\/])[A-Za-z0-9_.-]+")
REQUIRED_MATRIX_COLUMNS = {
    "id",
    "priority",
    "workflow",
    "initial_state",
    "steps",
    "expected_result",
    "dependencies",
    "target_studio_version",
    "evidence",
    "implementation_status",
    "test_status",
    "limitations",
    "owner",
}


def tracked_files() -> list[Path]:
    excluded = {".git", "target", "node_modules", "__pycache__"}
    files = []
    for path in ROOT.rglob("*"):
        if not path.is_file() or any(part in excluded for part in path.relative_to(ROOT).parts):
            continue
        files.append(path)
    return files


def check_whitespace(path: Path, content: bytes) -> list[str]:
    errors = []
    for number, line in enumerate(content.splitlines(), start=1):
        if line.endswith((b" ", b"\t")):
            errors.append(f"{path.relative_to(ROOT)}:{number}: trailing whitespace")
    return errors


def check_markdown_links(path: Path, content: str) -> list[str]:
    errors = []
    for match in MARKDOWN_LINK.finditer(content):
        target = match.group(1)
        if target.startswith("<") and target.endswith(">"):
            target = target[1:-1]
        if target.startswith(("https://", "http://", "mailto:", "#")):
            continue
        local_target = unquote(target.split("#", 1)[0].split("?", 1)[0])
        if not local_target:
            continue
        resolved = (path.parent / local_target).resolve()
        if not resolved.is_relative_to(ROOT) or not resolved.exists():
            errors.append(
                f"{path.relative_to(ROOT)}: unresolved local Markdown link {target!r}"
            )
    return errors


def check_matrix(path: Path) -> list[str]:
    errors = []
    with path.open(encoding="utf-8", newline="") as stream:
        reader = csv.DictReader(stream)
        columns = set(reader.fieldnames or [])
        missing = REQUIRED_MATRIX_COLUMNS - columns
        if missing:
            return [f"{path.relative_to(ROOT)}: missing columns {sorted(missing)}"]
        seen: set[str] = set()
        for line, row in enumerate(reader, start=2):
            identifier = row["id"].strip()
            if not identifier:
                errors.append(f"{path.relative_to(ROOT)}:{line}: empty workflow ID")
            elif identifier in seen:
                errors.append(f"{path.relative_to(ROOT)}:{line}: duplicate workflow ID {identifier}")
            seen.add(identifier)
            if row["priority"] not in {"P0", "P1", "P2"}:
                errors.append(f"{path.relative_to(ROOT)}:{line}: invalid priority {row['priority']!r}")
            if not row["workflow"].strip() or not row["expected_result"].strip():
                errors.append(f"{path.relative_to(ROOT)}:{line}: workflow and expected result are required")
    return errors


def main() -> int:
    errors: list[str] = []
    files = tracked_files()

    for path in files:
        relative = path.relative_to(ROOT)
        if path.suffix in TEXT_SUFFIXES:
            content = path.read_bytes()
            errors.extend(check_whitespace(path, content))
            try:
                text = content.decode("utf-8")
            except UnicodeDecodeError:
                errors.append(f"{relative}: text file is not UTF-8")
                continue
            if path.suffix == ".md":
                errors.extend(check_markdown_links(path, text))
            if path.suffix in {".py", ".rs", ".wgsl"}:
                for match in PERSONAL_PATH.finditer(text):
                    errors.append(
                        f"{relative}: possible author-specific path {match.group(0)!r}"
                    )
        elif path.suffix == ".json":
            try:
                json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as error:
                errors.append(f"{relative}: invalid JSON: {error}")

    matrix = ROOT / "docs/compatibility/compatibility-matrix.csv"
    if matrix.exists():
        errors.extend(check_matrix(matrix))
    else:
        errors.append("docs/compatibility/compatibility-matrix.csv: required workflow inventory is missing")

    python_files = sorted(path for path in files if path.suffix == ".py")
    for path in python_files:
        try:
            compile(path.read_text(encoding="utf-8"), str(path), "exec")
        except (OSError, SyntaxError) as error:
            errors.append(f"{path.relative_to(ROOT)}: Python syntax error: {error}")

    if errors:
        print("Repository checks failed:", file=sys.stderr)
        print("\n".join(f"- {error}" for error in errors), file=sys.stderr)
        return 1

    print(f"Repository checks passed ({len(files)} files inspected).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
