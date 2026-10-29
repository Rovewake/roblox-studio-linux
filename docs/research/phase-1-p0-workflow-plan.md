# Phase 1 P0 workflow investigation plan

**Status:** all eight P0 Linux behavior contracts are specified; the Phase 1 reverse-engineering gate remains open because REA's initial target searches timed out without usable findings. The project has no Windows test environment; do not schedule or gate this work on running Studio on Windows. A version-pinned package is available for read-only static inspection, but the implementation must continue to build binary-derived evidence through REA alongside technical documents, public format references, fixtures, and creator feedback. REA's Ghidra provider checks pass in an ephemeral environment, though the host remains outside REA's general documented Linux support boundary. See [the REA lab audit](phase-1-rea-lab.md) and the [P0 contracts](../compatibility/p0-contracts.md).

The workflows below come from the project's P0 scope. The contracts are proposed behavior for the Linux product, not claims about unobserved Studio behavior. The linked contract document records source evidence, fixture procedures, confidence, and unresolved details. Acceptance checks become executable as the relevant Linux subsystems are implemented. Use disposable local projects and do not include account credentials or private project data. Do not claim Windows validation.

| ID | Workflow | Unresolved behavior question | Evidence and Linux check to plan | Status |
|---|---|---|---|---|
| P0-01 | Launch and open a local project | Unspecified Studio startup details remain unknown; the Linux product contract is defined. | Clean-profile open/error/hash fixtures for supported files, invalid paths, and malformed input. | Contract specified; implementation/check pending |
| P0-02 | Create, save, close, and reopen a local project | Full format and version coverage remains unknown; safe round-trip behavior is defined for declared fields. | Semantic round-trip, unknown-field preservation, `rbx-dom` cross-check, and failed-write safety. | Contract specified; implementation/check pending |
| P0-03 | Navigate the hierarchy | Exact Studio ordering and UI details remain unknown; Linux identity and mutation rules are defined. | Duplicate-name, stable-identity, reparent, duplicate, delete, undo, and persistence fixtures. | Contract specified; implementation/check pending |
| P0-04 | Edit common properties | Exact Studio grouping and validation remain unknown; supported typed properties and persistence are defined. | Versioned metadata fixtures for valid/invalid values, attributes, tags, undo, and reopen. | Contract specified; implementation/check pending |
| P0-05 | Edit, analyze, and save Luau scripts | Studio-specific analyzer/API metadata remains unknown; core language handling and source preservation are defined. | Pinned Luau compiler/analyzer corpus with error, type mode, lint, Unicode, save, and multi-cursor cases. | Contract specified; implementation/check pending |
| P0-06 | Use the interactive viewport | Renderer and gizmo details remain unknown; supported geometry, navigation, transform, and profiling contracts are defined. | Deterministic geometry/CFrame/picking fixtures and Linux GPU frame-time profiles. | Contract specified; implementation/check pending |
| P0-07 | Undo and redo | Native Studio grouping and retention remain unknown; Linux command boundaries are defined. | Model snapshots across hierarchy, property, transform, script, multi-cursor, save, and branch invalidation. | Contract specified; implementation/check pending |
| P0-08 | Run and stop a basic local playtest | Exact engine runtime coverage remains unknown; local run snapshot and stop isolation are defined. | Runtime output/error fixture and model/file-hash checks before and after stop. | Contract specified; implementation/check pending |

## Evidence rules

- Keep direct observations separate from interpretation, inference, hypothesis, and unknowns.
- For static analysis, start with an installation inventory, then select one binary and one behavior question. Follow instructions, callers/callees, strings, constants, and references; pseudocode alone is not sufficient evidence.
- Use REA evidence only after its provider and target provenance are recorded. Include the REA/provider version, target path, Studio version, and SHA-256 of every analyzed binary.
- A passing Linux contract or fixture test does not establish parity with Windows Studio. Record its scope and avoid making claims beyond the evidence available.
