# ADR-0005: Add a bounded entry-script Luau debugger session

- **Status:** accepted
- **Date:** 2026-10-28

## Context

Phase 3 needs a debuggable language-only execution path. The pinned Luau VM exposes line breakpoints and returns `LUA_BREAK` from `lua_resume`, but the current compile-and-run service executes with `lua_pcall` and has only cancellation. A debugger must pause without blocking the UI, preserve cancellation and time limits while paused, and remain separate from Studio playtest behavior.

## Decision

- Use the debug API from pinned Luau 0.744. Keep the existing ordinary execution API unchanged.
- Add `ExecutionService::start_debug` as an off-UI worker API for up to 256 one-based breakpoints in the entry script chunk.
- Let Luau resolve a requested breakpoint to an executable source line and report the actual stopped line.
- At a stop, publish one bounded event, then wait outside the VM for an explicit resume or cancellation command. Do not block inside a VM callback waiting for UI input.
- Use the existing wall-clock execution deadline during pause waits. Paused time counts toward the deadline, and cancellation wakes the paused worker.
- Limit the first contract to breakpoints, continue, cancellation, and terminal results. Do not expose stepping, local/upvalue inspection, nested ModuleScripts, or coroutine debugging until their contracts and tests are independently established.
- Keep this runtime language-only. Do not imply Roblox Engine behavior or Studio playtest support. The VM limits remain defense-in-depth and do not make hostile scripts safe.

## Alternatives considered

- **Reuse `lua_pcall` and pause inside `debugbreak`:** rejected because it cannot return a resumable `LUA_BREAK` status and blocking in the callback would couple VM execution to UI scheduling.
- **Pause the worker without a bounded event contract:** rejected because repeated stops could queue unbounded editor messages.
- **Add step-through and local inspection immediately:** deferred because it expands the VM state and stack-lifetime contract beyond the first pause/resume test surface.
- **Treat this as Studio playtest debugging:** rejected because no Roblox Engine bindings, scheduler, services, project runtime, or playtest semantics are implemented.

## Consequences and limits

- A debug session is a separate worker job that shares the bounded execution worker pool, VM memory limit, captured-output limit, and wall-clock timeout.
- The event channel has capacity one; callers poll it and must receive a stop event before they can resume.
- A breakpoint with no executable line at or after it returns a debugger error. Luau may adjust a breakpoint to a later executable line.
- Only the entry chunk is covered. Step commands, locals/upvalues, nested function/module behavior, coroutine execution, and editor UI are not yet supported.
- No performance improvement or Studio parity claim is made. The contract is validated against synthetic scripts and the pinned Luau API only.

## Sources

- [Luau 0.744 debugger API](https://github.com/luau-lang/luau/blob/32c28089305f488f901aaf1dbf3fd1cb65fa3d11/VM/include/lua.h)
- [Luau 0.744 breakpoint implementation](https://github.com/luau-lang/luau/blob/32c28089305f488f901aaf1dbf3fd1cb65fa3d11/VM/src/ldebug.cpp)
- [Luau 0.744 debugger conformance test](https://github.com/luau-lang/luau/blob/32c28089305f488f901aaf1dbf3fd1cb65fa3d11/tests/Conformance.test.cpp)
