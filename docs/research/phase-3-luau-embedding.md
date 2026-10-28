# Phase 3 research: Luau compiler, analysis, and language-only embedding

## Question

How should the native Rust workspace get real Luau parsing, compilation, type analysis, linting, and a bounded initial runtime while keeping Roblox Engine behavior separate and avoiding fake APIs?

## Target and source provenance

- **Target:** upstream Luau source repository, not a Roblox Studio binary.
- **Version:** official Luau 0.744 release.
- **Git commit:** `32c28089305f488f901aaf1dbf3fd1cb65fa3d11`.
- **Source location:** `vendor/luau` Git submodule.
- **SHA-256:** not recorded for a Git checkout; the complete source tree is pinned by the immutable Git commit above. CI checks out the recorded gitlink recursively.
- **REA/provider versions:** not applicable. No REA analysis was needed for this question.
- **Host:** implementation uses a Linux CMake/C++ build and the workspace's pinned Rust toolchain. Windows execution is unavailable and not part of validation.

## Sources consulted

- [Luau 0.744 release](https://github.com/luau-lang/luau/releases/tag/0.744) and [pinned release commit](https://github.com/luau-lang/luau/commit/32c28089305f488f901aaf1dbf3fd1cb65fa3d11).
- [Official embedding API](https://luau.org/api/) and [sandbox guide](https://luau.org/sandbox/).
- Pinned upstream headers: `Ast/include/Luau/Parser.h`, `Ast/include/Luau/ParseResult.h`, `Compiler/include/Luau/Compiler.h`, `Bytecode/include/Luau/BytecodeBuilder.h`, `Analysis/include/Luau/Frontend.h`, `Analysis/include/Luau/FileResolver.h`, `Analysis/include/Luau/ConfigResolver.h`, `VM/include/lua.h`, and `VM/include/lualib.h`.
- Pinned upstream build targets in [`CMakeLists.txt`](https://github.com/luau-lang/luau/blob/0.744/CMakeLists.txt): `Luau.Ast`, `Luau.Analysis`, `Luau.Config`, `Luau.Compiler`, `Luau.Bytecode`, `Luau.VM`, and `Luau.Common`.
- [Official frontend API](https://github.com/luau-lang/luau/blob/0.744/Analysis/include/Luau/Frontend.h) and [source mode directives](https://luau.org/types/).

## Direct observations

- The official parser returns `ParseError` objects with Luau source locations. Its line and column coordinates are zero-based; this integration converts them to one-based lines/UTF-8 byte columns and keeps end positions exclusive.
- The compiler's `compileOrThrow` can compile a parsed source tree into a `BytecodeBuilder`; `CompileError` carries a source location. Debug level 2 retains local/upvalue debug names; optimization level 0 is selected for the first inspectable/debuggable implementation, not as a performance claim.
- The VM exposes an embedding interface, sandbox helper, custom allocator, and interrupt callback. The interrupt callback is a VM safepoint mechanism, and Luau coroutines are not OS threads; execution therefore runs on a Rust-owned OS worker thread.
- The official embedding documentation warns that sandboxing has not been formally proven. Memory caps, disabled libraries, and timeout interrupts are defense-in-depth, not a hostile-code security guarantee.
- No official Rust binding was found in the official repository/docs. The workspace uses a small C++ bridge and makes the public API Rust-owned.
- The official `Luau.Analysis` Frontend accepts a `FileResolver` and `ConfigResolver`, returns structured type-check errors and lint results, and supports source mode directives. This implementation supplies one in-memory module named `Script`, enables lint checks, and sets a cooperative 2-second per-module analysis limit. No filesystem or project-module resolver is supplied.
- The one-module resolver does not provide Roblox Engine type definitions or project `require` targets. Such references remain unknown/incomplete to analysis; passing analysis is not evidence that a script is valid in Studio.
- Luau is the language/compiler/runtime; it does not supply Roblox `Instance`, `game`, services, scheduler, physics, or Studio behavior.

## Interpretation and confidence

- **High confidence:** embedding the official pinned compiler/VM behind a narrow native bridge is the most direct way to test actual Luau syntax/compiler behavior in this Rust/Linux workspace.
- **High confidence:** source editing/persistence must remain independent from compilation; invalid source must remain saveable.
- **High confidence:** the first runtime should not invent Engine globals. Missing globals should fail with an explicit unsupported-API diagnostic.
- **Medium confidence:** the chosen selective standard-library set is adequate for a language-only first slice. This is a project-owned execution environment, not a claim about Studio's Luau environment; revise only through explicit contracts and tests.

## Current implementation

- `crates/studio-luau` exposes exact-source `ScriptDocument`, official parser/compiler and Frontend calls, stable diagnostics, and a bounded compile/run worker service.
- `vendor/luau` is pinned to release 0.744. The native bridge links the official parser, analysis, compiler, and VM static targets.
- The worker accepts at most 2 MiB source, allows a 1–256 MiB VM cap (64 MiB default), allows 1–60 seconds (2 seconds default), captures at most 1 MiB printed output, caps native runtime diagnostics at 16 KiB, and bounds concurrency at 1–16 jobs (2 default). The VM cap excludes source, compiler allocations, host output/message buffers, and total process RSS.
- The initial library set excludes `os` and `debug`; there is no filesystem access, Roblox global, project binding, scheduler, or playtest service.
- The current tests cover typed-syntax compilation, syntax-error locations, exact UTF-8/CRLF preservation, Frontend type errors and `--!nocheck`, lint output, undefined Engine globals, captured print output, missing `game`, timeout, cancellation, worker count, and input/limit validation.

## Confirmation and refutation tests

- Compile synthetic Luau samples and malformed source; assert diagnostic kind and stable one-based source positions.
- Preserve exact source bytes across edit operations, including Unicode and line-ending differences.
- Execute only deterministic synthetic scripts. Confirm print output, explicit missing-global errors, worker timeout/cancellation, memory-limit handling, and nonblocking job start.
- The [compiler baseline](../../benchmarks/results/README.md#phase-3-luau-compiler-baseline) was captured for deterministic 1k, 10k, and 50k-line source sizes with five warmups, 31 repetitions, and five independent processes. Add analysis, worker/VM startup, execution, and cancellation metrics as separate workloads before optimizing those paths. Do not claim an optimization until a diagnosed bottleneck is repeatably improved without contract regressions.
- The compiler baseline measures parsing/compilation only. Add separate analysis and project-module workloads before optimizing type-checking or resolution.

## Limitations

- No Windows machine or running Studio reference is available; there is no direct behavior/performance comparison.
- Current validation uses synthetic scripts and Linux builds only; it does not prove Roblox Engine compatibility or safe execution of adversarial scripts.
- Timeout/cancellation currently interrupts VM execution but does not stop parsing/compilation; source size is checked before a worker is scheduled, and compilation runs off the caller thread.
- Runtime diagnostics provide a line when Luau's error text reports one; source columns are not currently available for VM failures.
- Frontend analysis is a single-module check, not whole-project analysis. It has a cooperative timeout but no analysis memory cap; it must run off the UI thread. There is no module resolver, version-matched Engine reflection/bindings, formatter, completion, navigation, debugger, or UI integration.
