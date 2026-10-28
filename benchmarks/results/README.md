# Phase 0 headless baseline

**Date:** 2026-10-28
**Source revision:** [`4c09c4b`](https://github.com/Rovewake/roblox-studio-linux/commit/4c09c4b9973acad34c03c3857759cfc31491477f) (Phase 0 prototype and capture helper).

## Scope and setup

These measurements verify repeatability and native client/render paths without opening or focusing a window on the user's interactive desktop. They are not physical-display, desktop-compositor, hardware-GPU, or product SLO results.

- Host: NixOS 26.05, Linux 6.18.57, x86_64; Intel Core i3-1215U (6 cores / 8 threads), 11 GiB RAM.
- Workload: 10,000 hierarchy items, 20,000 generated source lines, and 2,000 instanced primitives; 1920×1080 virtual display.
- Build: Release profile, Rust 1.99.0, `Cargo.lock` pinned dependencies.
- Capture: 120 warmup frames, five independent 30-second runs per backend, process priority `nice 10`.
- Wayland server: Weston 16.0.0 headless with Pixman. X11 server: Xvfb 21.1.24.
- Renderer: Vulkan llvmpipe, LLVM 21.1.8, Mesa 26.1.8. Forcing the Intel ICD did not provide a surface compatible with either virtual display, so it is not represented in these results.
- Frame intervals are measured between UI callbacks and include event-loop and virtual compositor scheduling. They are not GPU execution or physical presentation timings. RSS is the reading at capture end, not peak RSS.

## Results

Each cell shows the median of five per-run statistics and the min–max range across those five runs. Values are milliseconds unless stated otherwise.

| Backend | Frame interval P50 | P95 | P99 | UI update CPU P50 / P95 | RSS at capture | Hitches >100 ms per run |
|---|---:|---:|---:|---:|---:|---:|
| Weston headless / Wayland | 17.07 (17.05–17.07) | 23.68 (23.14–25.35) | 27.55 (26.99–28.68) | 0.77 / 1.19 | 362.8 MiB (360.4–363.0) | 0 (0–0) |
| Xvfb / X11 | 17.18 (17.16–17.25) | 32.23 (17.69–38.37) | 43.08 (18.96–45.23) | 0.76 / 2.22 | 365.7 MiB (365.4–365.8) | 0 (0–7) |

Wayland's headless interval distribution was stable across these runs. X11's P95/P99 and hitch counts varied substantially, so the median must not be treated as a stable performance target. Both selected llvmpipe, not the host's Intel GPU. Neither result supports claims about visible frame pacing, actual desktop compositors, GPU drivers, or product workflows.

## Raw reports

| Backend | Repetitions |
|---|---|
| Weston headless / Wayland | [01](phase0-headless-wayland-01.json), [02](phase0-headless-wayland-02.json), [03](phase0-headless-wayland-03.json), [04](phase0-headless-wayland-04.json), [05](phase0-headless-wayland-05.json) |
| Xvfb / X11 | [01](phase0-headless-x11-01.json), [02](phase0-headless-x11-02.json), [03](phase0-headless-x11-03.json), [04](phase0-headless-x11-04.json), [05](phase0-headless-x11-05.json) |

The workloads and capture instructions are in [`benchmarks/README.md`](../README.md). No optimization was attempted from these data: they diagnose a software-rendering path only, and product performance budgets remain open until an end-to-end local-project workflow runs on representative Linux hardware.
