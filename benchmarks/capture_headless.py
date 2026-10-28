#!/usr/bin/env python3
"""Run the Phase 0 native spike against an isolated headless Linux display."""

from __future__ import annotations

import argparse
import json
import os
import select
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SPIKE = ROOT / "prototypes/phase-0-native-spike/target/release/phase-0-native-spike"


class VirtualDisplay:
    def __init__(self, backend: str, width: int, height: int) -> None:
        self.backend = backend
        self.width = width
        self.height = height
        self.process: subprocess.Popen[bytes] | None = None
        self.log = tempfile.TemporaryFile(mode="w+t", encoding="utf-8")
        self.runtime_directory: tempfile.TemporaryDirectory[str] | None = None
        self.environment = os.environ.copy()

    def start(self) -> None:
        if self.backend == "wayland":
            self._start_weston()
        else:
            self._start_xvfb()

    def _start_weston(self) -> None:
        binary = shutil.which("weston")
        if binary is None:
            raise FileNotFoundError("weston is required for a headless Wayland capture")
        runtime_base = os.environ.get("XDG_RUNTIME_DIR", tempfile.gettempdir())
        self.runtime_directory = tempfile.TemporaryDirectory(
            prefix="roblox-studio-linux-wayland-", dir=runtime_base
        )
        os.chmod(self.runtime_directory.name, 0o700)
        socket_name = "wayland-phase0"
        self.environment["XDG_RUNTIME_DIR"] = self.runtime_directory.name
        self.environment["WAYLAND_DISPLAY"] = socket_name
        self.environment["XDG_SESSION_TYPE"] = "wayland"
        self.environment.pop("DISPLAY", None)
        self.environment.pop("XAUTHORITY", None)
        self.process = subprocess.Popen(
            [
                binary,
                "--backend=headless",
                "--renderer=pixman",
                "--no-config",
                f"--socket={socket_name}",
                f"--width={self.width}",
                f"--height={self.height}",
                "--idle-time=0",
            ],
            env=self.environment,
            stdout=self.log,
            stderr=subprocess.STDOUT,
        )
        socket_path = Path(self.runtime_directory.name) / socket_name
        self._wait_for(lambda: socket_path.exists(), "Weston Wayland socket")

    def _start_xvfb(self) -> None:
        binary = shutil.which("Xvfb")
        if binary is None:
            raise FileNotFoundError("Xvfb is required for a headless X11 capture")
        read_fd, write_fd = os.pipe()
        try:
            self.process = subprocess.Popen(
                [
                    binary,
                    "-displayfd",
                    str(write_fd),
                    "-screen",
                    "0",
                    f"{self.width}x{self.height}x24",
                    "-nolisten",
                    "tcp",
                    "-noreset",
                    "-ac",
                ],
                pass_fds=(write_fd,),
                stdout=self.log,
                stderr=subprocess.STDOUT,
            )
        except Exception:
            os.close(read_fd)
            raise
        finally:
            os.close(write_fd)

        ready, _, _ = select.select([read_fd], [], [], 10.0)
        if not ready:
            os.close(read_fd)
            raise TimeoutError("Xvfb did not allocate a display within 10 seconds")
        with os.fdopen(read_fd, encoding="utf-8") as display_number_file:
            display_number = display_number_file.readline().strip().lstrip(":")
        if not display_number.isdigit():
            raise RuntimeError(f"Xvfb returned an invalid display number: {display_number!r}")
        self.environment["DISPLAY"] = f":{display_number}"
        self.environment["XDG_SESSION_TYPE"] = "x11"
        self.environment.pop("WAYLAND_DISPLAY", None)
        self.environment.pop("XAUTHORITY", None)

    def _wait_for(self, ready: Callable[[], bool], description: str) -> None:
        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline:
            if self.process is None or self.process.poll() is not None:
                raise RuntimeError(f"{description} failed to start:\n{self.read_log()}")
            if ready():
                return
            time.sleep(0.05)
        raise TimeoutError(f"timed out waiting for {description}")

    def read_log(self) -> str:
        self.log.seek(0)
        return self.log.read()

    def stop(self) -> None:
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        if self.runtime_directory is not None:
            self.runtime_directory.cleanup()
        self.log.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", choices=("wayland", "x11"), required=True)
    parser.add_argument("--metrics-out", type=Path, required=True)
    parser.add_argument("--capture-seconds", type=int, default=30)
    parser.add_argument("--warmup-frames", type=int, default=120)
    parser.add_argument("--nodes", type=int, default=10_000)
    parser.add_argument("--code-lines", type=int, default=20_000)
    parser.add_argument("--render-instances", type=int, default=2_000)
    parser.add_argument("--width", type=int, default=1920)
    parser.add_argument("--height", type=int, default=1080)
    parser.add_argument("--nice", type=int, default=10, choices=range(20))
    args = parser.parse_args()

    if args.capture_seconds < 1 or args.warmup_frames < 0:
        parser.error("capture duration must be positive and warmup frames cannot be negative")
    if args.width < 1 or args.height < 1:
        parser.error("virtual display dimensions must be positive")
    scheduler_nice = os.nice(args.nice)

    if not SPIKE.is_file():
        parser.error(f"Release spike binary not found: {SPIKE}")
    report_path = args.metrics_out
    if not report_path.is_absolute():
        report_path = ROOT / report_path
    report_path = report_path.resolve()
    if not report_path.is_relative_to(ROOT):
        parser.error("metrics output must stay inside this repository")
    if report_path.exists():
        parser.error(f"refusing to overwrite existing report: {report_path}")
    report_path.parent.mkdir(parents=True, exist_ok=True)

    virtual_display = VirtualDisplay(args.backend, args.width, args.height)
    spike_log = tempfile.TemporaryFile(mode="w+t", encoding="utf-8")
    app_command = [
        str(SPIKE),
        "--backend",
        args.backend,
        "--capture-seconds",
        str(args.capture_seconds),
        "--warmup-frames",
        str(args.warmup_frames),
        "--nodes",
        str(args.nodes),
        "--code-lines",
        str(args.code_lines),
        "--render-instances",
        str(args.render_instances),
        "--metrics-out",
        str(report_path),
    ]
    app: subprocess.Popen[bytes] | None = None
    try:
        virtual_display.start()
        app = subprocess.Popen(
            app_command,
            cwd=ROOT,
            env=virtual_display.environment,
            stdout=spike_log,
            stderr=subprocess.STDOUT,
        )
        timeout = max(60, args.capture_seconds + 180)
        return_code = app.wait(timeout=timeout)
        if return_code != 0:
            raise subprocess.CalledProcessError(return_code, app_command)
        if not report_path.is_file():
            raise RuntimeError("spike exited without writing its metrics report")

        report = json.loads(report_path.read_text(encoding="utf-8"))
        if report["capture_elapsed_ms"] < args.capture_seconds * 1_000:
            raise RuntimeError("spike capture ended before the requested measurement duration")
        if report["sampled_frames"] < 1:
            raise RuntimeError("spike did not record any measured frames")
        report.update(
            {
                "window_system": "Wayland" if args.backend == "wayland" else "X11",
                "display_server": "Weston headless with Pixman"
                if args.backend == "wayland"
                else "Xvfb",
                "headless": True,
                "visible_on_user_display": False,
                "display_size_px": [args.width, args.height],
                "scheduler_nice": scheduler_nice,
            }
        )
        report_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(f"recorded headless {args.backend} capture at {report_path.relative_to(ROOT)}")
        return 0
    except Exception:
        if app is not None and app.poll() is None:
            app.terminate()
            try:
                app.wait(timeout=5)
            except subprocess.TimeoutExpired:
                app.kill()
                app.wait(timeout=5)
        spike_log.seek(0)
        output = spike_log.read()
        if output:
            print(output, file=sys.stderr, end="")
        server_log = virtual_display.read_log()
        if server_log:
            print(server_log, file=sys.stderr, end="")
        raise
    finally:
        virtual_display.stop()
        spike_log.close()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, subprocess.SubprocessError, TimeoutError, ValueError, RuntimeError) as error:
        print(f"headless capture failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
