#!/usr/bin/env python3
"""Measure the pinned Luau compiler against deterministic generated source files."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import subprocess
import sys
import tempfile
from datetime import UTC, datetime
from pathlib import Path
from statistics import median


ROOT = Path(__file__).resolve().parents[1]
BENCHMARK = ROOT / "target/release/examples/phase3_compile_bench"
DEFAULT_SIZES = (1_000, 10_000, 50_000)


def output(command: list[str], *, cwd: Path = ROOT) -> str:
    result = subprocess.run(command, cwd=cwd, check=True, capture_output=True, text=True)
    return result.stdout.strip()


def cpu_model() -> str | None:
    try:
        for line in Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines():
            if line.lower().startswith("model name"):
                return line.split(":", 1)[1].strip()
    except OSError:
        return None
    return None


def distribution() -> dict[str, str]:
    try:
        release = platform.freedesktop_os_release()
    except (AttributeError, OSError):
        return {}
    return {
        key: release[key]
        for key in ("ID", "VERSION_ID", "PRETTY_NAME")
        if release.get(key)
    }


def generate_source(lines: int) -> str:
    if lines < 2:
        raise ValueError("line counts must be at least 2")
    return "local total = 0\n" + "total += 1\n" * (lines - 2) + "return total\n"


def measure(source_path: Path, warmups: int, iterations: int) -> dict[str, object]:
    result = subprocess.run(
        [str(BENCHMARK), str(source_path), str(warmups), str(iterations)],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout.splitlines()[-1])


def summarize(cases: list[dict[str, object]]) -> list[dict[str, object]]:
    grouped: dict[int, list[dict[str, object]]] = {}
    for case in cases:
        grouped.setdefault(int(case["source_lines"]), []).append(case)

    summaries = []
    for lines, group in sorted(grouped.items()):
        summary = {}
        for name in ("source_bytes", "bytecode_bytes", "p50_ms", "p95_ms", "peak_rss_kib"):
            values = sorted(float(case[name]) for case in group)
            summary[name] = {
                "median": median(values),
                "min": values[0],
                "max": values[-1],
            }
        summaries.append(
            {
                "source_lines": lines,
                "independent_process_samples": len(group),
                "metrics_across_processes": summary,
            }
        )
    return summaries


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sizes", default=",".join(map(str, DEFAULT_SIZES)))
    parser.add_argument("--warmups", type=int, default=5)
    parser.add_argument("--iterations", type=int, default=31)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "benchmarks/results/phase3-luau-baseline.json",
    )
    args = parser.parse_args()

    if not sys.platform.startswith("linux"):
        parser.error("the Phase 3 benchmark reads peak RSS from /proc and targets Linux")
    try:
        sizes = [int(value) for value in args.sizes.split(",")]
    except ValueError:
        parser.error("--sizes must be a comma-separated list of positive integers")
    if not sizes or any(size < 2 for size in sizes):
        parser.error("--sizes must contain integers of at least 2 lines")
    if args.warmups < 1 or args.iterations < 30 or args.samples < 5:
        parser.error("use at least 1 warmup, 30 iterations, and 5 independent processes")

    # A lower scheduler priority reduces contention with the interactive desktop.
    try:
        nice_value = os.getpriority(os.PRIO_PROCESS, 0)
        if nice_value < 10:
            os.nice(10 - nice_value)
            nice_value = os.getpriority(os.PRIO_PROCESS, 0)
    except OSError as error:
        parser.error(f"could not set benchmark niceness: {error}")

    output(["cargo", "build", "--locked", "--release", "-p", "studio-luau", "--example", "phase3_compile_bench"])
    luau_commit = output(["git", "-C", "vendor/luau", "rev-parse", "HEAD"])
    cases: list[dict[str, object]] = []
    with tempfile.TemporaryDirectory(prefix="studio-luau-bench-") as temporary_dir:
        temporary_path = Path(temporary_dir)
        for lines in sizes:
            source = generate_source(lines)
            source_bytes = source.encode("utf-8")
            source_path = temporary_path / f"generated-{lines}.luau"
            source_path.write_bytes(source_bytes)
            digest = hashlib.sha256(source_bytes).hexdigest()
            for sample in range(1, args.samples + 1):
                print(f"Measuring {lines} lines, process {sample}/{args.samples}", flush=True)
                result = measure(source_path, args.warmups, args.iterations)
                result["source_sha256"] = digest
                result["independent_sample"] = sample
                cases.append(result)

    memory_kib = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES") // 1024
    report = {
        "captured_at_utc": datetime.now(UTC).isoformat(timespec="seconds"),
        "source_revision": output(["git", "rev-parse", "HEAD"]),
        "working_tree_dirty": bool(output(["git", "status", "--porcelain"])),
        "luau_source_commit": luau_commit,
        "host": {
            "os": platform.system(),
            "distribution": distribution(),
            "kernel": platform.release(),
            "architecture": platform.machine(),
            "cpu_model": cpu_model(),
            "logical_cpus": os.cpu_count(),
            "memory_kib": memory_kib,
            "scheduler_nice": nice_value,
            "cargo": output(["cargo", "--version"]),
            "rustc": output(["rustc", "--version"]),
            "cmake": output(["cmake", "--version"]).splitlines()[0],
            "cxx": output(["c++", "--version"]).splitlines()[0],
        },
        "method": {
            "profile": "release",
            "workload": "generated Luau script with the requested source line count; local accumulator plus repeated arithmetic assignments",
            "operation": "parse and compile to bytecode through studio-luau",
            "source_files": "temporary deterministic synthetic files; no project or user scripts",
            "warmups_per_process": args.warmups,
            "iterations_per_process": args.iterations,
            "independent_process_samples_per_workload": args.samples,
            "percentile_method": "nearest rank",
            "peak_rss": "Linux /proc/self/status VmHWM for each benchmark process",
            "scheduler_priority": "nice 10",
            "limits": "synthetic compiler baselines; not Studio parity, editor latency, or product SLO measurements",
        },
        "summary": summarize(cases),
        "cases": cases,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    try:
        label = args.output.relative_to(ROOT)
    except ValueError:
        label = args.output
    print(f"Wrote {label}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
