# Native Roblox Studio for Linux

A project to port Roblox Studio to a native Linux application. The long-term goal is to cover Studio workflows independently, with behavior and compatibility verified; running the Windows program through a compatibility layer is outside the project's goal.

This is intended to become a distributable tool for Linux creators beyond the repository owner. It must work across its documented Linux support matrix and must not depend on the author's machine, account, or local file layout.

## Status

The project is in the planning and research stage. There is no application code, build, or release yet. The [roadmap](ROADMAP.md) defines phases, exit criteria, quality and performance metrics, and open technical decisions.

## Principles

- Use original Studio on Windows as a behavior reference, not a performance ceiling.
- Guide investigation with REA, traceable evidence, and differential tests.
- Prevent silent data loss and avoid optimizations without diagnosis and before/after measurements.
- Keep the implementation and runtime native to Linux.
- Use portable defaults and user configuration; do not hard-code developer-specific paths, credentials, accounts, or hardware assumptions.

## Project documentation

- [ROADMAP.md](ROADMAP.md): phases, scope, acceptance criteria, metrics, and technical sources.
- [AGENTS.md](AGENTS.md): agent instructions for engineering, research, quality, optimization, and collaboration.
