# Native Roblox Studio for Linux

A project to independently reimplement Roblox Studio workflows as a native Linux application. Compatibility claims will be limited to technical documentation, static evidence, public format specifications, fixtures, and Linux tests. This project has no Windows test environment, so it cannot make direct behavior or performance comparisons with a running Studio. Running the Windows program through a compatibility layer is outside the project's goal.

This is intended to become a distributable tool for Linux creators beyond the repository owner. It must work across its documented Linux support matrix and must not depend on the author's machine, account, or local file layout.

## Status

The REA lab and evidence-based contracts for all eight P0 workflows are complete. The project is still pre-implementation: there is no native application code, build, executable acceptance suite, or release. Phase 0 architecture decisions, Linux baselines, and the native rendering prototype remain ahead of product implementation. The [roadmap](ROADMAP.md) defines phases, exit criteria, quality and performance metrics, and open technical decisions.

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
