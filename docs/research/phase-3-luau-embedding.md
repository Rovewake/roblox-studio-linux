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
- The official, version-pinned [Roblox API dump](https://setup.rbxcdn.com/channel/common/version-7062aec9dbe74b14-API-Dump.json) for Studio `0.745.0.7451137` (`version-7062aec9dbe74b14`), SHA-256 `0962f777e3085487e368ed019c35bdb07cd02f6659aa7212dc2ad6e24bb41efe`, 7,669,358 bytes, 944 classes, and 654 enums. The full raw file stays in the external XDG cache; only a reviewed subset of declarations is generated for the project.
- [Roblox Engine API reference](https://create.roblox.com/docs/reference/engine) — public technical documentation for classes, data types, enums, globals, properties, functions, events, and callbacks. It is an independent check on the category and meaning of public API entries, but does not by itself identify the selected Studio build.
- [ModuleScript reference](https://create.roblox.com/docs/reference/engine/classes/ModuleScript) and [Luau globals reference](https://create.roblox.com/docs/reference/engine/globals/LuaGlobals) — public technical descriptions and examples for `require`, `script.Parent`, and static string require paths.
- [StyLua README](https://github.com/JohnnyMorganz/StyLua/blob/v2.6.0/README.md) — documents a Luau syntax mode, line-ending controls, and a `--verify` mode that reparses output and compares ASTs. StyLua is used for explicit formatting previews, but its documentation cautions that verification can have false positives and false negatives; it is not a semantic guarantee.
- The pinned [StyLua 2.6.0 library API](https://docs.rs/stylua/2.6.0/stylua_lib/) and [crate manifest](https://github.com/JohnnyMorganz/StyLua/blob/v2.6.0/Cargo.toml) — expose the `stylua_lib` API and a Luau-only feature build; the crate is MPL-2.0 licensed.

## Direct observations

- The official parser returns `ParseError` objects with Luau source locations. Its line and column coordinates are zero-based; this integration converts them to one-based lines/UTF-8 byte columns and keeps end positions exclusive.
- The compiler's `compileOrThrow` can compile a parsed source tree into a `BytecodeBuilder`; `CompileError` carries a source location. Debug level 2 retains local/upvalue debug names; optimization level 0 is selected for the first inspectable/debuggable implementation, not as a performance claim.
- The VM exposes an embedding interface, sandbox helper, custom allocator, and interrupt callback. The interrupt callback is a VM safepoint mechanism, and Luau coroutines are not OS threads; execution therefore runs on a Rust-owned OS worker thread.
- The official embedding documentation warns that sandboxing has not been formally proven. Memory caps, disabled libraries, and timeout interrupts are defense-in-depth, not a hostile-code security guarantee.
- No official Rust binding was found in the official repository/docs. The workspace uses a small C++ bridge and makes the public API Rust-owned.
- The official `Luau.Analysis` Frontend accepts a `FileResolver` and `ConfigResolver`, returns structured type-check errors and lint results, and supports source mode directives. The single-script API supplies one in-memory module named `Script`, enables lint checks, and sets a cooperative 2-second per-module analysis limit.
- Project analysis accepts a caller-provided immutable hierarchy snapshot and resolves unique local `ModuleScript` targets for documented static paths: `script.Parent`/child access, constant `WaitForChild` and `FindFirstChild` calls, `game:GetService("...")`, and `./`, `../`, `@self/`, and `@game/` string paths. Dynamic expressions, duplicate names, remote asset IDs, and non-ModuleScript dependencies are not guessed. The selected entry has a separate Frontend root identity so a normal Script/LocalScript cannot be loaded as a `require` dependency. This is static resolver behavior, not a DataModel implementation.
- The selected package's `ReflectionMetadata.xml` is UI grouping/categorization metadata, not a typed Engine API catalog. The version-matched API dump is the source for generated Engine type declarations; its raw JSON remains outside the repository, and generated declarations are explicitly partial.
- Luau is the language/compiler/runtime; it does not supply Roblox `Instance`, `game`, services, scheduler, physics, or Studio behavior.
- StyLua has a Luau parser mode, but formatting rewrites text and its own verification is not proof of semantic preservation. The editor should only apply an explicit user-requested format action after showing a diff or providing undo; auto-format-on-save is not enabled by this milestone.

## Interpretation and confidence

- **High confidence:** embedding the official pinned compiler/VM behind a narrow native bridge is the most direct way to test actual Luau syntax/compiler behavior in this Rust/Linux workspace.
- **High confidence:** source editing/persistence must remain independent from compilation; invalid source must remain saveable.
- **High confidence:** the first runtime should not invent Engine globals. Missing globals should fail with an explicit unsupported-API diagnostic.
- **Medium confidence:** the chosen selective standard-library set is adequate for a language-only first slice. This is a project-owned execution environment, not a claim about Studio's Luau environment; revise only through explicit contracts and tests.

## Current implementation

- `crates/studio-luau` exposes exact-source `ScriptDocument`, official parser/compiler and Frontend calls, stable diagnostics, and a bounded compile/run worker service.
- `analyze_project` validates a bounded project snapshot (100,000 nodes, 512 ModuleScripts, 2 MiB per source, 16 MiB combined source, 256-byte IDs, and 512-byte names) before crossing the FFI boundary. It returns diagnostics attributed to project-local node IDs, caps static resolver work at 250,000 child/descendant visits, and uses a cooperative 15-second analysis deadline plus Luau's 2-second per-module limit. Work-budget exhaustion produces an `AnalysisLimit` diagnostic rather than silently implying complete analysis. It is a blocking API and must run off the UI thread; there is no Frontend memory cap.
- Project snapshots use static local module resolution only. Identical names in the searched tree are treated as ambiguous and remain unresolved; the selected entry is loaded only as the Frontend root unless it is itself a ModuleScript. Runtime lookup, dynamic paths, asset-ID requires, remote services, cyclic ModuleScript execution, and project mutation are not implemented.
- `complete_project` uses Luau's pinned Frontend autocomplete on the same project snapshot and partial Engine type definitions. It returns up to 256 sorted, categorized suggestion labels at a validated zero-based UTF-8 byte position. Suggestions never modify the source; there is no editor UI or replacement-range support yet.
- `find_local_definition` uses the pinned Luau AST query to map a local-variable or parameter use to its declaration range, including shadowed parameters. It deliberately returns no result for globals, properties, or cross-module symbols; no editor UI consumes the result yet.
- `format_preview` pins StyLua 2.6.0 with its Luau feature, disables require sorting, uses full AST verification, and matches uniform LF/CRLF line endings. It checks source and output with the pinned official Luau compiler, refuses mixed or bare-CR input, enforces the 2 MiB source limit, and returns a proposed string without mutating `ScriptDocument`. StyLua is an additional parser and has no cancellation API; call it off the UI thread. The future editor must present the diff and apply it only as one undoable, explicit user action.
- `docs/research/engine-api-surface.json` pins the exact API dump for Studio `0.745.0.7451137` / upload `version-7062aec9dbe74b14`, including its SHA-256, size, schema version, and class/enum counts. `scripts/fetch_studio_api_dump.py` stores the raw dump only in the external XDG cache. `scripts/generate_studio_engine_types.py` emits the reviewed allowlist as `roblox-engine-0.745.0.7451137.luau`; it includes a small `Instance`/service/workspace surface and is not a complete API binding.
- Project analysis loads the version-pinned partial type surface and Luau builtins so `game`, `script`, `workspace`, selected `Instance` members, and standard Luau globals have known types. `GetService` remains typed as general `Instance`; the stub does not model dynamic children, services beyond the selected hierarchy, events, callbacks, scheduler, physics, or runtime behavior. Single-script `analyze` remains language-only and has no Engine types.
- `vendor/luau` is pinned to release 0.744. The native bridge links the official parser, analysis, compiler, and VM static targets.
- The worker accepts at most 2 MiB source, allows a 1–256 MiB VM cap (64 MiB default), allows 1–60 seconds (2 seconds default), captures at most 1 MiB printed output, caps native runtime diagnostics at 16 KiB, and bounds concurrency at 1–16 jobs (2 default). The VM cap excludes source, compiler allocations, host output/message buffers, and total process RSS.
- The initial library set excludes `os` and `debug`; there is no filesystem access, Roblox global, project binding, scheduler, or playtest service.
- The current tests cover typed-syntax compilation, syntax-error locations, exact UTF-8/CRLF preservation, Frontend type errors and `--!nocheck`, lint output, missing Engine globals in language-only analysis/runtime, captured print output, timeout, cancellation, worker count, project snapshot validation, static module resolution, module-attributed diagnostics, pinned Engine type loading, editor completion from those types, local definition navigation including shadowing, UTF-8 cursor validation, and input/limit validation.

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
- Frontend analysis and project analysis are separate APIs. Project analysis resolves only the documented static subset above, has a cooperative timeout and a cumulative resolver-work cap, but no analysis memory cap, and must run off the UI thread. The checked-in Engine surface is version-matched but partial; it is type metadata, not runtime bindings. Autocomplete, local definition navigation, and formatting preview are backend APIs only. StyLua verification is not proof of semantic preservation; a second parser can differ from the pinned Luau parser. There is no cross-module/global navigation, debugger, or native editor integration yet.
