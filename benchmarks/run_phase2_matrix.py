#!/usr/bin/env python3
"""Run the synthetic Phase 2 project-format benchmark matrix serially."""

from __future__ import annotations

import argparse
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
FORMATS = ("rbxl", "rbxlx", "rbxm", "rbxmx")


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


def temporary_filesystem() -> str | None:
    try:
        lines = output(["df", "-T", tempfile.gettempdir()]).splitlines()
    except (OSError, subprocess.CalledProcessError):
        return None
    if len(lines) < 2:
        return None
    fields = lines[-1].split()
    return fields[1] if len(fields) >= 2 else None


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


def benchmark_case(file_format: str, instances: int, iterations: int) -> dict[str, object]:
    command = [
        "cargo",
        "bench",
        "-p",
        "studio-project",
        "--bench",
        "format_io",
        "--",
        "--format",
        file_format,
        "--instances",
        str(instances),
        "--iterations",
        str(iterations),
    ]
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(
            f"benchmark failed for {file_format} / {instances} instances:\n"
            f"{result.stdout}\n{result.stderr}"
        )
    for line in reversed(result.stdout.splitlines()):
        if line.startswith("{"):
            return json.loads(line)
    raise RuntimeError(f"benchmark emitted no JSON result for {file_format} / {instances}")


def summarize_cases(cases: list[dict[str, object]]) -> list[dict[str, object]]:
    grouped: dict[tuple[str, int], list[dict[str, object]]] = {}
    for case in cases:
        key = (str(case["format"]), int(case["requested_instances"]))
        grouped.setdefault(key, []).append(case)

    metric_names = (
        "file_bytes",
        "parse_p50_ms",
        "parse_p95_ms",
        "verified_encode_p50_ms",
        "verified_encode_p95_ms",
        "atomic_save_p50_ms",
        "atomic_save_p95_ms",
        "peak_rss_kib",
    )
    summaries = []
    for (file_format, instances), group in sorted(grouped.items()):
        metrics = {}
        for name in metric_names:
            values = sorted(float(case[name]) for case in group)
            metrics[name] = {
                "median": median(values),
                "min": values[0],
                "max": values[-1],
            }
        summaries.append(
            {
                "format": file_format,
                "requested_instances": instances,
                "independent_samples": len(group),
                "metrics_across_processes": metrics,
            }
        )
    return summaries


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--sizes",
        default="1000,5000",
        help="comma-separated Part counts (default: 1000,5000)",
    )
    parser.add_argument("--iterations", type=int, default=31)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "benchmarks/results/phase2-format-baseline.json",
    )
    args = parser.parse_args()

    if not sys.platform.startswith("linux"):
        parser.error("the Phase 2 benchmark reads peak RSS from /proc and currently targets Linux")
    try:
        sizes = [int(value) for value in args.sizes.split(",")]
    except ValueError:
        parser.error("--sizes must be a comma-separated list of positive integers")
    if not sizes or any(size <= 0 for size in sizes):
        parser.error("--sizes must contain positive integers")
    if args.iterations < 30:
        parser.error("--iterations must be at least 30")
    if args.samples < 5:
        parser.error("--samples must be at least 5 independent processes")

    cases = []
    for size in sizes:
        for file_format in FORMATS:
            for sample in range(1, args.samples + 1):
                print(
                    f"Measuring {file_format} with {size} Parts, sample {sample}/{args.samples}",
                    flush=True,
                )
                result = benchmark_case(file_format, size, args.iterations)
                result["independent_sample"] = sample
                cases.append(result)

    memory_kib = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES") // 1024
    report = {
        "captured_at_utc": datetime.now(UTC).isoformat(timespec="seconds"),
        "source_revision": output(["git", "rev-parse", "HEAD"]),
        "working_tree_dirty": bool(
            output(["git", "status", "--porcelain"])
        ),
        "host": {
            "os": platform.system(),
            "distribution": distribution(),
            "kernel": platform.release(),
            "architecture": platform.machine(),
            "cpu_model": cpu_model(),
            "logical_cpus": os.cpu_count(),
            "memory_kib": memory_kib,
            "temporary_filesystem": temporary_filesystem(),
            "cargo": output(["cargo", "--version"]),
            "rustc": output(["rustc", "--version"]),
        },
        "method": {
            "profile": "release",
            "workload": "one Workspace with generated Part children; Anchored and CFrame per Part",
            "operations": ["parse", "verified encode", "atomic save with backup and re-open"],
            "save_target": "fresh system temporary directory",
            "warmups": {"parse": 5, "verified_encode": 5, "atomic_save": 5},
            "iterations_per_operation": args.iterations,
            "independent_process_samples_per_workload": args.samples,
            "percentile_method": "nearest rank",
            "limits": "synthetic fixtures only; not Studio parity or product SLO measurements",
        },
        "summary": summarize_cases(cases),
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
