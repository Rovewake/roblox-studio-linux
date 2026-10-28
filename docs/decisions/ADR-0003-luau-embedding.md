# ADR-0003: Embed the official Luau runtime behind a Rust-owned API

- **Status:** accepted
- **Date:** 2026-10-28

## Context

Phase 3 needs actual Luau parsing, compilation, type analysis, linting, and a bounded language runtime. A Lua-compatible substitute would not guarantee Luau syntax, bytecode, or analyzer behavior. The workspace is Rust, but the official Luau project exposes C++ parser/compiler/analysis APIs and a C-style VM API rather than an official Rust binding. Luau itself does not implement Roblox Engine or Studio services.

## Decision

- Pin the official [Luau 0.744 release](https://github.com/luau-lang/luau/releases/tag/0.744) at commit [`32c28089305f488f901aaf1dbf3fd1cb65fa3d11`](https://github.com/luau-lang/luau/commit/32c28089305f488f901aaf1dbf3fd1cb65fa3d11) as the `vendor/luau` Git submodule. The source is distributed under the upstream MIT license; do not copy proprietary Studio material into the source tree.
- Keep a narrow C++ bridge around upstream parser/compiler and VM APIs. Expose script documents, structured diagnostics, compilation, and execution through the Rust `studio-luau` crate.
- Use `Luau.Analysis`'s `Frontend` with an in-memory resolver for one named `Script` module. Honor Luau source modes and return structured type and lint diagnostics. Do not present the analysis as project-wide: `require` targets and Roblox Engine definitions are not available.
- Preserve the script text exactly, including invalid source and line endings. Report one-based lines and UTF-8 byte columns, with zero for missing coordinates.
- Execute jobs on a bounded pool of OS worker threads. Defaults are 2 seconds, 64 MiB VM memory, and two active jobs; public limits allow 1–60 seconds, 1–256 MiB, and 1–16 workers. Limit source to 2 MiB, captured `print` output to 1 MiB, and native runtime diagnostics to 16 KiB. The VM memory limit covers VM allocations only, not source, compiler AST/bytecode, host buffers, or total process RSS.
- Timeout and cancellation are checked at VM execution safepoints. The upstream parse/compiler path has no cancellation callback; a compile is bounded by the 2 MiB source limit and runs off the caller thread, but cancellation requested during compilation is observed only if/when VM execution starts.
- Expose selected language libraries only. Do not implement Roblox Engine globals by returning placeholder values. An unknown global produces an explicit error that Engine APIs are not implemented.
- Use recursive submodule checkout in CI and in documented user clone instructions.

## Alternatives considered

- **Use a generic Lua runtime:** rejected because it would not provide the pinned Luau parser/compiler behavior.
- **Use an unofficial Rust binding as the architecture boundary:** rejected for this first slice because there is no official Rust binding and the reviewed candidate did not track the selected upstream release. Revisit only with an explicit upstream-version, safety, and API-coverage audit.
- **Implement Roblox globals as stubs:** rejected because that would manufacture behavior and make successful execution look like Studio compatibility.
- **Run scripts synchronously in the calling thread:** rejected because script execution must not block future UI work.

## Consequences and limits

- The native build requires CMake and a C++17 compiler in addition to Rust.
- The analyzer honors `--!strict`, `--!nonstrict`, and `--!nocheck` and reports Luau type errors and lint findings for a single in-memory module. Its 2-second per-module timeout is cooperative, has no analysis memory cap, and cannot resolve project `require` calls or Roblox Engine definitions. There is no formatter, navigation, completion, debugger, `Instance`, service layer, scheduler, physics, or Studio playtest integration yet.
- Compiler/runtime jobs are bounded as described above, but those limits do not cover all host allocations. Type analysis runs separately from the execution service and must also be dispatched off the UI thread by a future shell.
- The worker execution path is language-only. It is not a Roblox sandbox or security boundary. Upstream documents sandboxing as not formally proven; time and memory limits reduce runaway work but do not establish safety against hostile scripts or native implementation defects. Do not run hostile or remote scripts until OS process isolation is implemented and tested.
- No optimization claim is made. Compiler latency, worker startup, memory use, and cancellation latency need repeatable Linux baselines before performance changes are considered.
- This decision establishes compatibility only with the pinned Luau language/compiler behavior tested by our synthetic fixtures. It makes no direct parity claim about running Windows Studio.

## Verification required before expanding scope

- Keep exact-source fixtures for UTF-8 text, CRLF/LF, trailing newlines, valid Luau syntax, syntax/type errors, lint findings, source modes, runtime errors, and missing Engine globals.
- Run the crate's tests in CI with the pinned submodule and record Linux compiler/build versions.
- Before adding module resolution, navigation, completion, formatting, or debugger behavior, evaluate the corresponding official APIs and define project-level contracts. Before exposing Engine bindings, obtain version-matched API metadata and add contract fixtures; do not infer missing signatures from the stale bundled reflection database.
