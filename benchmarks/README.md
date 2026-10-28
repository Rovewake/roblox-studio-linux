# Linux workload and measurement plan

**Status:** repeatable workloads specified; the initial native spike establishes a provisional host baseline, not product SLOs.

## Workloads

Every run uses a versioned disposable fixture and records build commit, compiler/toolchain, distribution, kernel, CPU/RAM, GPU, driver, desktop session, window backend, renderer backend, display size/scale, power mode, warmup, repetitions, and raw results.

| ID | Workload | Initial fixture/action | Primary measures |
|---|---|---|---|
| BENCH-01 | Cold start to first UI | Launch from a clean app state; stop after first rendered frame | Process launch-to-first-frame P50/P95; exit/crash status |
| BENCH-02 | Warm start | Repeat with caches warm and the same build | Launch-to-first-frame P50/P95; RSS |
| BENCH-03 | Large hierarchy | 100,000 generated nodes arranged under generated models; scroll/focus a fixed sequence | UI-update CPU time, action latency, frame intervals, RSS |
| BENCH-04 | Long source file | 50,000 generated Luau-like lines, deterministic caret and scroll positions | UI-update CPU time, scroll/input latency, frame intervals, RSS |
| BENCH-05 | 3D viewport | 1,000 / 10,000 / 50,000 instanced primitive cubes and a fixed camera sequence | Frame interval P50/P95/P99, >100 ms hitches, CPU, RSS, GPU time only when supported |
| BENCH-06 | Project open/save | Small/medium/large generated supported place fixtures; cold and warm cache | Open/save P50/P95, peak RSS, CPU, serialized size, semantic equality |
| BENCH-07 | Property edit and selection | Fixed select/edit sequence against a large hierarchy | Input-to-visible-update P50/P95, frame intervals, model correctness |
| BENCH-08 | Playtest | Deterministic local fixture, repeated Run/Stop | Start/stop latency, CPU/RSS, crash rate, model/file-hash invariance |

The Phase 0 spike implements a smaller diagnostic workload: 10,000 hierarchy items, 20,000 generated source lines, and 2,000 instances. It does not yet reach the BENCH-03/04 full fixture sizes or the BENCH-05 10,000/50,000 instance cases. It records UI-update CPU duration separately from callback intervals. Intervals include event-loop, render, virtual compositor, and scheduling delays; they are not GPU execution time or physical-display presentation. The first spike does not yet collect per-process CPU totals, peak RSS, or GPU timestamp queries. Add those only with a justified, portable measurement method.

## Repetition and analysis rules

1. Use a Release build with identical code, toolchain, fixture counts, UI layout, and display size for comparisons.
2. Warm the app and shader pipeline with at least 120 frames, then start the capture timer and collect at least 30 seconds per sample. The report records requested and observed capture duration and total frame count.
3. Collect at least five independent samples per backend/workload after warmup. Preserve each raw JSON record; summarize per-run distributions and across-run median/range.
4. Report P50, P95, and P99 where sample counts permit, plus hitches above 100 ms, RSS, and CPU/GPU/I/O measures relevant to the hypothesis.
5. Compare only measurements from the same machine, driver, session, power mode, and display configuration. Repeat after any environment change.
6. Do not use a single machine to claim broad Linux support. A GPU or session enters the release matrix only after direct testing.
7. Treat a result as a baseline, not a target. An optimization requires a diagnosed bottleneck and a repeatable gain above measurement variation without functional or data-integrity regression.

Use `python3 benchmarks/capture_headless.py --backend wayland --metrics-out benchmarks/results/run-name.json` to launch the Release spike in a private Weston headless compositor. Use `--backend x11` for an isolated Xvfb display. Both modes run without opening or focusing a window on the developer's desktop. The helper lowers its own process priority to `nice 10` by default (`--nice` can change it) and records the applied priority, backend, virtual display size, selected renderer adapter, and raw timing. Weston uses its Pixman compositor; the spike still selects and reports its own WGPU adapter. These captures validate native window/render paths and deterministic workloads, but their frame intervals are not user-facing display timings and cannot establish support for a physical compositor or monitor. Do not treat them as product SLOs or compare them with interactive measurements.

## Initial product limits

The [ROADMAP performance table](../ROADMAP.md#metrics-and-measurement-method) contains proposed release limits, not measurements. The native spike's first measurements are provisional and must not be used as P0 product SLOs. Set absolute product SLOs after the first native vertical slice can open, edit, save, reopen, and playtest a project end to end.

## Results

Store machine-specific reports under `benchmarks/results/`. Use a generic host label and omit usernames, serial numbers, project paths, and private data. Each result must include the exact measurement method and limitations.
