# ADR-0004: Provide explicit Luau formatting previews with StyLua

- **Status:** accepted
- **Date:** 2026-10-28

## Context

The code-tools phase needs a Luau formatter, but formatting rewrites user text. The project must keep invalid or unsupported source editable and must not silently change scripts during save. StyLua 2.6.0 provides a Rust library API with a Luau parser mode and AST verification, but it uses `full_moon`, a parser separate from the pinned official Luau compiler.

## Decision

- Pin the StyLua crate at exactly `2.6.0`, disable default features, and enable only its `luau` feature. The crate is distributed under MPL-2.0; include its license notice in dependency metadata for any distributed build.
- Provide `format_preview(&ScriptDocument)` as a backend API that returns proposed text without mutating the source document.
- Require both the pinned official Luau compiler and StyLua's Luau parser to accept the input. Use StyLua's full AST verification, then require the official Luau compiler to accept the output.
- Disable require sorting. Match uniform LF or CRLF input; reject mixed and bare-CR line endings rather than silently normalize them.
- Enforce the existing 2 MiB script source limit. Callers must run formatting away from the UI thread because StyLua has no cancellation or deadline API.
- The future editor must present the proposed diff and apply it only as one explicit undoable edit. Never format automatically on save.

## Alternatives considered

- **Mutate the document on save:** rejected because formatting is a destructive text rewrite and StyLua verification is not a semantic guarantee.
- **Use the StyLua CLI as an external runtime dependency:** rejected for this backend because availability and version would vary by Linux user and machine.
- **Treat StyLua AST verification as proof of semantic preservation:** rejected because upstream documents false positives and false negatives, and its parser is not the official Luau parser.
- **Implement a formatter from scratch or reuse a different Lua formatter:** rejected for this slice because StyLua has an explicit Luau mode and a maintained public formatter contract.

## Consequences and limits

- The build includes a second Luau-capable parser and the StyLua library's dependency tree. The supported source set is the intersection accepted by the pinned Luau compiler and StyLua's configured Luau parser.
- Full AST verification and the official compiler check reduce risk but do not prove runtime equivalence or Engine compatibility.
- Formatting has no cancellation hook; the 2 MiB source limit bounds input size but is not a time bound. The application must use an off-UI worker and may need process isolation if measured worst-case latency is unacceptable.
- `format_preview` is not editor integration. Preview display, explicit acceptance, and one-step undo remain Phase 3/4 work.
- No performance improvement is claimed. Any future optimization requires a diagnosed formatting bottleneck and same-host measurements.

## Sources

- [StyLua 2.6.0 README](https://github.com/JohnnyMorganz/StyLua/blob/v2.6.0/README.md)
- [StyLua 2.6.0 library API](https://docs.rs/stylua/2.6.0/stylua_lib/)
- [StyLua 2.6.0 crate manifest and license metadata](https://github.com/JohnnyMorganz/StyLua/blob/v2.6.0/Cargo.toml)
