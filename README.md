# Native Roblox Studio for Linux

A project to independently reimplement Roblox Studio workflows as a native Linux application. Compatibility claims will be limited to technical documentation, static evidence, public format specifications, fixtures, and Linux tests. This project has no Windows test environment, so it cannot make direct behavior or performance comparisons with a running Studio. Running the Windows program through a compatibility layer is outside the project's goal.

This is intended to become a distributable tool for Linux creators beyond the repository owner. It must work across its documented Linux support matrix and must not depend on the author's machine, account, or local file layout.

## Status

The Phase 0 architecture and measurement gate, Phase 1 REA/evidence gate, and Phase 2 declared synthetic data-layer gate are complete. Phase 2's guarded Rust `studio-project` crate handles a tested subset of `.rbxl`, `.rbxm`, `.rbxlx`, and `.rbxmx`, with atomic local saves and recovery copies; local and GitHub Actions quality gates passed, and the Linux format baseline is recorded. Phase 3 is in progress: the separate `studio-luau` crate embeds pinned official Luau 0.744 parsing, compilation, type analysis, and linting behind a Rust API, preserves source as written, and runs language-only jobs on bounded worker threads. The analyzer has no Roblox Engine definitions or project `require` resolver, and this is not editor integration or Studio playtest. The native prototype, Linux build checks, and headless Wayland/X11 baseline passed GitHub Actions; the contracts cover all eight P0 workflows. This remains pre-product: no Studio workflow is implemented, no release environment is qualified, and no app is published. The prototype measures a generated hierarchy, long source buffer, and instanced renderer; it is not a usable Studio replacement. See the [roadmap](ROADMAP.md), [Linux support matrix](docs/platform/linux-support-matrix.md), and [measurement results](benchmarks/results/README.md).

## Principles

- Use technical documentation, static package inspection, public format specifications, traceable fixtures, and creator feedback to define behavior; mark unsupported assumptions as unknown.
- Verify contracts with Linux tests and independent implementations where available. Do not claim direct Windows parity.
- Measure performance against Linux workload targets and previous port builds; do not claim the port is faster than Studio.
- Prevent silent data loss and avoid optimizations without diagnosis and before/after measurements.
- Keep the implementation and runtime native to Linux.
- Use portable defaults and user configuration; do not hard-code developer-specific paths, credentials, accounts, or hardware assumptions.
- Luau compiler/type diagnostics and the language-only runner do not imply Roblox Engine or Studio compatibility. The analyzer has no Roblox Engine definitions; missing APIs must stay visibly unsupported until implemented and tested.

## Building

Clone with submodules because the official Luau source is pinned under `vendor/luau`:

```sh
git clone --recurse-submodules https://github.com/Rovewake/roblox-studio-linux.git
```

Build and test with the pinned Rust toolchain, CMake, a C++17 compiler, and a CMake build tool such as GNU Make available:

```sh
cargo test --locked --workspace
```

## Project documentation

- [ROADMAP.md](ROADMAP.md): phases, scope, acceptance criteria, metrics, and technical sources.
- [AGENTS.md](AGENTS.md): agent instructions for engineering, research, quality, optimization, and collaboration.
- [ADR-0002](docs/decisions/ADR-0002-project-format-adapter.md) and [Phase 2 format coverage](docs/research/phase-2-format-coverage.md): project format boundary, evidence, limits, and fixture provenance.
