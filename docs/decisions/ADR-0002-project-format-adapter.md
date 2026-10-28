# ADR-0002: Guarded local project-format adapter

**Status:** accepted for the Phase 2 data-layer slice; broad file compatibility remains unproven.
**Date:** 2026-10-28.

## Context

The editor needs a native Linux project tree and local read/write support before a UI or runtime can be connected. Roblox `.rbxl` and `.rbxlx` place files and `.rbxm` and `.rbxmx` model files have public technical descriptions and independent Rust serializers. The selected serializer does not represent every current or future Roblox value, and some decoder paths skip metadata, unknown types, or unresolved references. A successful decode alone therefore cannot be treated as proof that a file is safe to overwrite.

The repository has no Windows test environment. Phase 2 must rely on technical sources, the pinned dependency source, synthetic fixtures, and Linux tests; it cannot claim a comparison with a running Studio.

## Decision

- Add a Rust workspace package named `studio-project`, with a project-owned API around `WeakDom`. Keep filesystem behavior, limits, errors, file format selection, instance IDs, and semantic equality checks outside the third-party API.
- Pin `rbx_binary` 3.0.1, `rbx_xml` 3.0.1, `rbx_dom_weak` 4.2.0, `rbx_reflection` 7.0.0, and the bundled reflection database 3.0.1. All are MIT-licensed packages from the `rbx-dom` project.
- Select the bundled reflection database explicitly in both decoders and encoders and enable `rbx_reflection_database/debug_always_use_bundled` so constructors do not consult an author-specific local cache or `RBX_DATABASE` override before the explicit selection takes effect.
- Preflight XML and binary bytes before invoking `rbx-dom`. Reject unsupported encodings and known silent-loss cases, including unsupported XML property tags, XML `Meta` and `External` entries, unresolved references/shared strings, non-empty binary metadata, unknown binary chunk/type IDs, duplicate structural entries, excessive sizes, and binary properties the pinned decoder does not serialize.
- Validate each accepted binary property payload through its complete encoded length, reject non-canonical boolean/font data that the upstream decoder would normalize, and bound sequence, content, and aggregate instance counts before allocation. XML sequence keypoints and serialized output also have explicit limits.
- Use `ReadUnknown` and `WriteUnknown` for XML property names and supported XML value tags. Binary unknown property names use the wire type's default `Variant` and can be retained when the type is supported.
- Keep hierarchy mutation behind `ProjectDocument` methods. Do not expose mutable `WeakDom` access that could create orphan instances outside the serialized root tree.
- Generate instance referents inside the project adapter; do not accept caller-supplied `InstanceBuilder` referents that could collide with existing IDs and replace instances.
- Verify each serialized document by reopening it and comparing a canonical semantic snapshot. This validates only the supported in-memory model; it does not prove Studio compatibility.
- Save by writing and syncing a same-directory staged file, reopening and comparing it, and syncing the parent directory after commit. Reject symlinks and non-regular destinations.
- For an existing destination, atomically exchange the staged file and destination, compare the displaced inode's identity, metadata, and bytes with the captured snapshot, and move the verified displaced inode to a unique adjacent backup without copying it. If the snapshot changed, atomically restore the destination when the staged output is still intact; preserve the displaced file at its temporary path if safe rollback is no longer possible.
- For a destination that was absent at the snapshot, commit with no-replace rename semantics so a concurrent create is not overwritten. Report concurrent changes as a save conflict.
- Centralize Rust dependencies and the lockfile at the workspace root. Run format, tests, Clippy, release build, repository metadata checks, and the isolated headless Wayland/X11 smoke paths in CI.

## Consequences

- The first supported corpus is intentionally synthetic and narrow. A place file with an unsupported value may fail to open; callers must not remove that guard to make the file appear to load.
- The native data layer can open, inspect, edit through its API, serialize, and atomically save its declared subset, but there is not yet a graphical editor or a general-purpose Studio project importer.
- Save performs an encode and a full parse/semantic verification, so save latency and peak memory may be higher than an unchecked serializer. Capture a Linux baseline before considering any optimization.
- The adjacent backup consumes disk space. Its path is returned to the caller; cleanup policy belongs to a later user-facing recovery workflow.
- Linux `renameat2` exchange and no-replace semantics are required for the race-resistant commit path. A concurrent save to the same path is rejected when the destination snapshot changes; the user-facing editor will need to explain the conflict and let the creator compare or retry.
- Output is semantic, not byte-for-byte identical. Formatting, referent identifiers, and compression may differ while represented tree data remains equal.
- A bundled reflection database is reproducible but can lag newer file properties. Unsupported data must remain an explicit error until tests justify extending the adapter.

## Rejected alternatives

- Use `rbx-dom` directly as the app-wide domain model: rejected because its format coverage and defaults are not the editor's compatibility contract.
- Silently rely on upstream defaults: rejected because unknown fields and metadata may be discarded.
- Execute the Windows Studio reference or use Wine/Proton/Vinegar: rejected because this project is a native Linux reimplementation and has no Windows test environment.
- Implement all file encodings from scratch in this slice: deferred because the pinned library already supplies the core value serializers; the preflight boundary retains the option to replace individual components if coverage evidence requires it.

## Sources

- [Roblox Creator Hub: place files](https://create.roblox.com/docs/projects/place-files) — high-level `.rbxl`/`.rbxlx` place-file descriptions.
- [rbx-dom README and property coverage](https://github.com/rojo-rbx/rbx-dom/blob/master/README.md) — model, serializers, and documented type gaps.
- [rbx-dom binary format reference](https://github.com/rojo-rbx/rbx-dom/blob/master/docs/binary.md) and [XML format reference](https://github.com/rojo-rbx/rbx-dom/blob/master/docs/xml.md) — unofficial community specifications.
- [Phase 2 implementation evidence and limitations](../research/phase-2-format-coverage.md).
