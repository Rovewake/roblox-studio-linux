# Native Roblox Studio for Linux

A project to independently reimplement Roblox Studio workflows as a native Linux application. Compatibility claims will be limited to technical documentation, static evidence, public format specifications, fixtures, and Linux tests. This project has no Windows test environment, so it cannot make direct behavior or performance comparisons with a running Studio. Running the Windows program through a compatibility layer is outside the project's goal.

This is intended to become a distributable tool for Linux creators beyond the repository owner. It must work across its documented Linux support matrix and must not depend on the author's machine, account, or local file layout.

## Status

The Phase 0 architecture and measurement gate and the Phase 1 REA/evidence gate are complete. Phase 2 is in review: a Rust `studio-project` crate now handles a guarded synthetic subset of `.rbxl`, `.rbxm`, `.rbxlx`, and `.rbxmx`, with atomic local saves and recovery copies. This does not yet establish compatibility with arbitrary Studio projects. The native prototype, Linux build checks, and headless Wayland/X11 baseline passed GitHub Actions; the contracts cover all eight P0 workflows. This remains pre-product: no Studio workflow is implemented, no release environment is qualified, and no app is published. The prototype measures a generated hierarchy, long source buffer, and instanced renderer; it is not a usable Studio replacement. See the [roadmap](ROADMAP.md), [Linux support matrix](docs/platform/linux-support-matrix.md), and [headless measurements](benchmarks/results/README.md).

## Principles

- Use technical documentation, static package inspection, public format specifications, traceable fixtures, and creator feedback to define behavior; mark unsupported assumptions as unknown.
- Verify contracts with Linux tests and independent implementations where available. Do not claim direct Windows parity.
- Measure performance against Linux workload targets and previous port builds; do not claim the port is faster than Studio.
- Prevent silent data loss and avoid optimizations without diagnosis and before/after measurements.
- Keep the implementation and runtime native to Linux.
- Use portable defaults and user configuration; do not hard-code developer-specific paths, credentials, accounts, or hardware assumptions.

## Project documentation

- [ROADMAP.md](ROADMAP.md): phases, scope, acceptance criteria, metrics, and technical sources.
- [AGENTS.md](AGENTS.md): agent instructions for engineering, research, quality, optimization, and collaboration.
- [ADR-0002](docs/decisions/ADR-0002-project-format-adapter.md) and [Phase 2 format coverage](docs/research/phase-2-format-coverage.md): project format boundary, evidence, limits, and fixture provenance.
