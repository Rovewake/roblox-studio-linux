# Phase 0 baseline and scope

**Date:** 2026-10-28
**Status:** discovery and five-run-per-backend headless baseline complete; final GitHub Actions verification pending.

## Repository state before the spike

- Public repository: `Rovewake/roblox-studio-linux`, default branch `main`.
- No product workflow implementation, build system, GitHub Actions workflow, compatibility matrix, benchmark corpus, or release artifact existed. This phase adds a Rust rendering spike, Cargo build, GitHub Actions workflow, and generated headless reports; none is a usable Studio replacement.
- The versioned reference acquisition script and metadata-only Studio baseline already existed from Phase 1.
- GitHub Actions runs and status checks were absent; `main` had no branch protection at the time of audit. The repository is public and Issues are enabled. See the Phase 0 audit recorded for commit `1348389` and the workflow added in this phase.

## Host inventory

| Item | Observation |
|---|---|
| Distribution / kernel | NixOS 26.05; Linux `6.18.57`; x86_64 |
| CPU / memory | Intel Core i3-1215U, 6 cores / 8 threads; 11 GiB RAM; no swap |
| GPU | Intel PCI ID `8086:46B3`; kernel driver `i915`; `/dev/dri/renderD128` is accessible |
| Graphics userspace | Mesa 26.1.8. Headless WGPU captures selected Vulkan llvmpipe (LLVM 21.1.8); the Intel `i915` adapter was not available on the virtual display surfaces. |
| Desktop | Niri on Wayland; `WAYLAND_DISPLAY=wayland-1`; `DISPLAY=:0` exposes Xwayland |
| Tools initially missing from PATH | Rust/Cargo, C compiler, pkg-config, Vulkan diagnostics, X11/Wayland diagnostic utilities |
| Temporary build approach | Nix store packages for Rust/Cargo 1.99.0, compiler and X11/Wayland/Vulkan dependencies; no system profile changes |

This inventory is a developer-machine observation, not a Linux release support claim. Xwayland is not equivalent to a native Xorg session. To avoid interrupting the user's active desktop, no repeated benchmark used the physical Niri/Xwayland session.

## Scope and product priorities

- P0 is the eight local workflows in the [P0 evidence-based contracts](../compatibility/p0-contracts.md).
- P1 contains larger project/format coverage, advanced transforms, debugging, assets, terrain, animation/audio, preferences/recovery, and selected plugins.
- P2 contains collaboration, account/remote services, publishing, advanced asset integration, complex plugin APIs, and workflows later prioritized from creator feedback.
- The complete inventory, dependency links, evidence, test state, limitations, and ownership live in [`compatibility-matrix.csv`](../compatibility/compatibility-matrix.csv).
- This phase establishes x86_64 as the first architecture and Ubuntu 26.04 LTS as the first broad distribution candidate. NixOS 26.05 is the current development host. No distribution or GPU is yet declared supported.

## Measurement baseline

The Phase 0 spike adds deterministic stress loads for 10,000 hierarchy items, 20,000 generated source lines, and 2,000 instanced primitives. It records UI update CPU duration, callback interval distributions, hitches above 100 ms, selected adapter metadata, capture duration, and RSS at capture time. Per-frame sample data does not include GPU execution timestamps or peak RSS. The 10-run summary and raw reports are in [`benchmarks/results/`](../../benchmarks/results/README.md); the workloads and interpretation limits are in the [measurement plan](../../benchmarks/README.md).

Captures used the Release build with 120 warmup frames, 30-second sampling windows, five runs per backend, 1920×1080 virtual displays, `nice 10`, and the same generated workload. Wayland used Weston 16.0.0 headless with Pixman; X11 used Xvfb 21.1.24. Both selected llvmpipe via Vulkan on Mesa 26.1.8. These runs establish a repeatable software-rendering baseline only; they do not measure hardware GPU performance, actual desktop compositors, or product workflows. Product SLOs remain uncalibrated until a native project workflow exists.

## Technical evidence and decision inputs

- [winit 0.30.13](https://docs.rs/winit/0.30.13/winit/) exposes native Linux window/event backends and explicit Wayland/X11 event-loop selection.
- [egui/eframe 0.37.0](https://docs.rs/eframe/0.37.0/eframe/) provides a Rust native UI path; [egui-wgpu](https://docs.rs/egui-wgpu/0.37.0/egui_wgpu/) supports custom WGPU rendering callbacks inside UI regions.
- [wgpu](https://github.com/gfx-rs/wgpu) provides a safe Rust graphics API. Its backend and GPU timing support vary by adapter; report the actual chosen backend and do not assume timestamp queries are available.
- GTK4 offers Linux-native widgets and Wayland/X11 support through GDK, while Qt provides mature Linux platform plugins. They remain UI alternatives until the spike and creator feedback establish a fit.
- Luau's [official C API](https://luau.org/api/) can embed the language VM; it does not provide Roblox engine services, scheduler, or physics.
- The [XDG Base Directory specification](https://specifications.freedesktop.org/basedir/latest/) defines user-scoped locations for configuration, state, data, cache, and runtime files.
- Canonical lists Ubuntu 26.04 LTS standard security maintenance through May 2031. The matching [GitHub Actions runner image](https://github.com/actions/runner-images/tree/main/images/ubuntu) is suitable for headless build checks, not GPU certification.

## Open limitations

- The physical Intel GPU cannot present through Weston headless or Xvfb, so both headless runs use llvmpipe. Forcing the Intel ICD returned no compatible Vulkan surface. Physical GPU/driver behavior remains unmeasured in this phase.
- There is no native Xorg session available for direct validation. The automated X11 test uses an isolated Xvfb server; it is not an Xwayland or native Xorg desktop test.
- X11 headless P95/P99 intervals varied substantially between runs; preserve that range and do not use the median as a stable product target.
- There is one physical GPU and one workstation. No performance or compatibility conclusion generalizes to AMD, NVIDIA, other distributions, laptops, or desktop sessions.
- The spike is a measurement instrument and architecture probe; it is not a Roblox Studio replacement or a user-ready port.
