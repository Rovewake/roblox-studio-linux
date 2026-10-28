# Phase 2 project-format coverage and evidence

**Date:** 2026-10-28
**Status:** implementation and synthetic tests are being integrated; the phase acceptance gate is pending CI and the baseline capture.

## Question

Can a pinned Rust data-format library support a small native Linux project-data layer for Roblox place/model files while converting known silent-loss behavior into controlled errors?

## Target and implementation versions

- File encodings: binary model/place version 0 and XML model/place version 4.
- `rbx_binary`: 3.0.1; `rbx_xml`: 3.0.1; `rbx_dom_weak`: 4.2.0.
- `rbx_reflection`: 7.0.0; `rbx_reflection_database`: 3.0.1 (bundled database selected explicitly).
- Repository package: `studio-project` 0.1.0.
- REA/provider and Studio executable hash: not applicable to this format-adapter question. No Windows executable is run or dynamically observed.
- Host, GPU, driver, backend, and desktop session: see the Linux host in [Phase 0 baseline](phase-0-baseline.md); this phase's parser tests do not depend on graphics or display state.

## Sources

1. [Roblox Creator Hub place files](https://create.roblox.com/docs/projects/place-files) describes local `.rbxl` as binary and `.rbxlx` as XML. It provides high-level format context, not a full compatibility contract.
2. [`rbx-dom` README and property coverage](https://github.com/rojo-rbx/rbx-dom/blob/master/README.md) documents the shared Rust DOM and the current serializer coverage table.
3. [`rbx-dom` binary format v0](https://github.com/rojo-rbx/rbx-dom/blob/master/docs/binary.md) and [XML format v4](https://github.com/rojo-rbx/rbx-dom/blob/master/docs/xml.md) are explicitly unofficial community specifications based on independent implementations and observed files.
4. Pinned crate source was downloaded from crates.io and inspected locally from the versions above. `Cargo.lock` records registry checksums. Relevant source files include `rbx_binary/src/deserializer/state.rs`, `rbx_binary/src/serializer/state.rs`, `rbx_xml/src/deserializer.rs`, `rbx_xml/src/types/mod.rs`, and `rbx_reflection_database/src/lib.rs`.

## Observations

- The upstream DOM represents instance class, name, typed properties, referents, and ordered children.
- XML's `ReadUnknown` path still skips unknown XML value tags. XML metadata is read into decoder-local state, not into the returned DOM. Unknown `External` entries are ignored. A non-null XML reference without a matching item referent is left unset; missing shared-string hashes are also left unresolved.
- Binary decoding logs and skips unrecognized chunk names, unsupported value type IDs, and missing property type bytes. For a recognized property wire type and an unknown property name, it chooses a default `Variant` type; this allows some future fields to remain representable. Known non-serializing reflected properties can be ignored. The binary serializer currently emits no file metadata.
- The pinned binary decoder reads a declared number of property values but does not generally require a property chunk to be fully consumed. Extra bytes can therefore disappear on serialization. It also converts some non-canonical booleans and invalid font enum values to normalized values. The adapter validates complete payload lengths and rejects those lossy encodings before decode.
- The reflection database helper can consult a local `.rbxreflection` file or `RBX_DATABASE` override by default. The adapter selects the pinned bundled database explicitly and enables the crate feature that forces constructors to use it, making behavior deterministic without first touching developer-local configuration.
- `rbx-dom`'s README coverage matrix lists types that are not implemented in some serializers. The adapter does not claim support beyond its tests and guards.
- The binary and XML serializers may rewrite referents, compression, and XML formatting. The equality check therefore compares a semantic tree snapshot, not bytes.

## Interpretation and confidence

- **High confidence:** the upstream paths described above exist in the pinned source; this is directly observed in the crate source and covered by negative tests.
- **High confidence for the tested subset:** synthetic values survive parse → serialize → parse through the pinned adapter, subject to the fixture suite.
- **Low confidence outside the tested subset:** arbitrary Studio projects may contain newer formats, property types, metadata, external references, or combinations absent from the synthetic corpus. No Windows or representative user-project data has been compared.
- **Not inferred:** passing tests do not show that the Linux reimplementation matches a running Windows Studio.

## Implemented guard and test strategy

- Enforce input/output byte, decompressed byte, aggregate instance, chunk, per-property sequence/content, and XML nesting limits before the upstream parser allocates from attacker-controlled framing.
- Reject unsupported XML value tags, XML `Meta`/`External`, unknown attributes, duplicate referents/properties/shared-string IDs, unresolved references/shared strings, and XML versions other than 4.
- Reject unsupported binary chunks/type IDs, non-empty `META`, duplicate structural chunks/entries, non-UTF-8 reflected `String` values, and structural lengths inconsistent with declared counts. Permit arbitrary bytes for unknown binary string properties because the pinned decoder maps an unknown type-0x01 property to `BinaryString`.
- Use an explicitly bundled reflection database, keep hierarchy mutation behind `ProjectDocument` APIs so callers cannot create unserialized orphan instances, then verify serialization by reopening output and comparing instance order, names, classes, supported properties, and internal references.
- Save to a same-directory temporary, sync and re-open it, atomically exchange it with an existing destination, verify the displaced inode against the snapshot, then move that inode into a unique adjacent `.bak-<pid>-<id>` backup and sync the directory. New destinations use no-replace rename semantics. If a concurrent change is detected after exchange, restore the displaced destination when the staged output is intact; otherwise retain the displaced file at its temporary path for recovery.
- Tests use a repository-authored XML place fixture plus generated in-memory models. No proprietary Studio data, Windows-generated file, or user project is committed.

## Open questions and limits

- What common real projects use value types outside the current upstream coverage table?
- How should app-facing APIs represent raw binary and unknown XML values in a long-term domain model without inheriting `rbx-dom`'s type decisions?
- Are there legitimate XML attributes or binary chunks beyond the explicitly supported versions that need to be preserved rather than rejected?
- The first `ReadLimits` defaults (512 MiB input, 512 MiB decompressed, one million instances/chunks, XML depth 1024) are conservative engineering limits, not measured maximum Studio project sizes or product SLOs.
- The `rbx-dom` bundled reflection snapshot may trail Studio. A dependency or database update requires re-running fixture, semantic round-trip, negative-case, and performance baselines.
- No actual arbitrary Studio project has been validated. Do not call the crate a generally compatible Studio project editor.

## Confirmation/refutation tests

- Run the full root workspace suite on a headless Linux environment; verify XML and binary paths for place/model extensions, cross-references, duplicate names, supported values, and binary opaque bytes.
- Mutate format headers/chunks and XML structures to confirm unsupported inputs fail before save and do not alter the original destination.
- Repeat the fixture corpus after changing any pinned serializer, reflection database, decoder option, or file-guard rule.
- The committed XML fixture's provenance and SHA-256 are recorded in [the fixture manifest](phase-2-fixture-manifest.md).

## Limitations and redacted data

All committed project bytes are synthetic and authored for this repository. The public upstream test corpus was not copied. No account data, cookies, credentials, user files, Roblox Studio binaries, or proprietary assets are present in the fixtures.
