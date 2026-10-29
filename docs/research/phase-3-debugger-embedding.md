# Phase 3 research: Luau debugger sessions

## Question

Can the pinned official Luau VM support a resumable, bounded line-breakpoint session for language-only scripts without blocking a future editor loop or changing the existing run API?

## Target and source provenance

- **Target:** official Luau VM source.
- **Version:** Luau 0.744.
- **Git commit:** `32c28089305f488f901aaf1dbf3fd1cb65fa3d11` in the `vendor/luau` submodule.
- **Target SHA-256:** not applicable to a repository target; the exact source revision is pinned by the Git commit above.
- **REA/provider versions:** not used; the VM API and conformance test were read directly.
- **Host / GPU / driver / graphics backend / desktop session:** not applicable to this static source/API investigation; no display or Studio process was inspected.

## Sources consulted

- Pinned [VM debugger API](https://github.com/luau-lang/luau/blob/32c28089305f488f901aaf1dbf3fd1cb65fa3d11/VM/include/lua.h).
- Pinned [breakpoint implementation](https://github.com/luau-lang/luau/blob/32c28089305f488f901aaf1dbf3fd1cb65fa3d11/VM/src/ldebug.cpp).
- Pinned [debugger conformance test](https://github.com/luau-lang/luau/blob/32c28089305f488f901aaf1dbf3fd1cb65fa3d11/tests/Conformance.test.cpp).
- Pinned resume implementation: `vendor/luau/VM/src/ldo.cpp`.

## Direct observations

- `lua_breakpoint` takes a Luau function and a one-based line. The implementation chooses the next executable line and returns it, or returns `-1` when no executable line is available.
- Luau's debug callback may call `lua_break`; `lua_resume` then returns `LUA_BREAK`, and another `lua_resume` continues the suspended thread. The upstream conformance test exercises the callback and resume sequence.
- Luau documents that `interrupt` may be set safely from another thread while the VM is running, while other callbacks must be configured while the VM is idle.
- `lua_getlocal` pushes values onto the Luau stack. Exposing those values requires additional stack and value-lifetime rules; the first session does not call it.

## Interpretation and confidence

- **High confidence:** the existing `lua_pcall` execution path cannot be resumed after a debugger stop; a debugger path needs `lua_resume` and an explicit `LUA_BREAK` loop.
- **High confidence:** a future UI must not block inside `debugbreak`. The native worker can return from `lua_resume`, publish a bounded stop event, and wait on synchronized resume/cancel commands outside the VM.
- **High confidence:** the existing timeout must continue while paused. Otherwise leaving a session paused would bypass its wall-clock resource limit.
- **Medium confidence:** the first useful contract can be restricted to the entry script chunk. Nested functions, repeated same-line hits, and coroutine behavior need separate fixtures before broader claims are made.

## Open hypotheses

- Breakpoint behavior in nested functions, repeated loop hits, and coroutine stacks may require more state handling than this entry-chunk contract; no claim is made until those cases have dedicated fixtures.
- The engine-backed Studio debugger may have distinct scheduling and stepping behavior; this language-only VM service is not evidence about it.

## Current implementation

- `ExecutionService::start_debug` starts a worker in the same bounded pool as ordinary language-only execution.
- It accepts at most 256 one-based entry-script breakpoint entries; duplicate lines are deduplicated. Luau's resolved executable line is returned as a bounded `BreakpointHit` event.
- The caller polls `DebugExecutionHandle::try_event`, then explicitly calls `resume` or `cancel`. The native wait is outside VM execution, and cancellation wakes it.
- The selected wall-clock timeout is used by the paused wait as well as normal VM safepoints. Captured output and VM allocator limits are shared with normal runs.
- Tests cover a breakpoint hit and continuation, duplicate line deduplication, cancellation while paused, timeout while paused, output before the stop, breakpoint input limits, a request with no later executable line, and discarding a queued stop event after terminal completion.

## Confirmation and refutation tests

- Run a deterministic top-level script with a breakpoint; confirm the reported line, output before the stop, continuation, and final output.
- Cancel while paused and confirm the worker returns `Cancelled` without resume.
- Let the wall-clock deadline expire while paused and confirm `TimedOut`.
- Request zero, excessive, duplicate, and non-executable lines; verify invalid inputs are rejected or reported as debugger errors without memory corruption or a stuck worker.
- Keep all debug API tests in the language-only runtime. These do not establish Engine behavior, Studio playtest semantics, or parity with running Studio.

## Result on Linux and independent references

- The pinned upstream conformance test confirms the `lua_breakpoint` / `lua_break` / `lua_resume` stop-and-resume sequence.
- The native bridge and Rust handle tests pass on Linux with the pinned Luau source. They cover only the explicit cases listed above and do not compare against a running Studio.
- The repository's `studio-luau` unit suite currently passes 40 tests; this count is a snapshot and will change as coverage grows.

## Limitations

- No stepping, stack inspection, locals/upvalues, coroutine debugging, nested ModuleScripts, Engine bindings, or editor UI is implemented.
- The debugger runs scripts in the existing in-process language-only VM. Its resource limits and Luau sandbox are defense-in-depth, not a security boundary for hostile input.
- No Windows or live Studio reference exists, so this verifies only the pinned Luau API contract on Linux.
- No credentials, cookies, private account data, or user project files were used in this investigation.
