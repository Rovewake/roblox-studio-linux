# Phase 1 P0 workflow investigation plan

**Status:** evidence-based behavior contracts are pending. The project has no Windows test environment; do not schedule or gate this work on running Studio on Windows. A version-pinned package is available for static inspection, and technical documents, public format references, fixtures, and creator feedback are the available evidence sources. REA's Ghidra provider checks pass in an ephemeral environment, though the host remains outside REA's general documented Linux support boundary. See [the REA lab audit](phase-1-rea-lab.md).

The workflows below come from the project's P0 scope. Their questions identify unknown behavior; they are not claims about current Studio. For each one, record the technical source or fixture, exact Linux reproduction steps, expected result, confidence, and limitations. Use disposable local projects and do not include account credentials or private project data. Do not claim Windows validation.

| ID | Workflow | Unresolved behavior question | Evidence and Linux check to plan | Status |
|---|---|---|---|---|
| P0-01 | Launch and open a local project | What startup and project-open behavior is guaranteed by available technical evidence? | Define startup/open telemetry and expected errors; use a clean Linux profile and a small generated fixture to verify launch and project state. | Needs evidence-based contract |
| P0-02 | Create, save, close, and reopen a local project | Which project formats and data structures can the implementation safely round trip? | Use a disposable fixture with a known hierarchy and script; test parse/save/reopen semantic equality and unknown-field preservation against public format references and independent serializers. | Needs evidence-based contract |
| P0-03 | Navigate the hierarchy | Which ordering, identity, selection, and parent/child rules are specified by the data model and public technical references? | Define these rules in the project contract; test tree changes and selection state against generated fixtures. Mark undocumented UI details unknown. | Needs evidence-based contract |
| P0-04 | Edit common properties | Which property types, values, and validation rules are documented for supported classes? | Build property fixtures from public API metadata; verify validation, serialization, undo, and save/reopen behavior against the declared contract. | Needs evidence-based contract |
| P0-05 | Edit, analyze, and save Luau scripts | Which language diagnostics are defined by Luau, and which Roblox APIs are documented for supported classes? | Use the Luau compiler/test corpus for language behavior; separate engine APIs from language behavior and report unsupported APIs explicitly. | Needs evidence-based contract |
| P0-06 | Use the interactive viewport | Which camera, coordinate, transform, and rendering rules are specified by public technical references or existing fixtures? | Use deterministic geometry and transform fixtures; record expected matrices and viewport metrics on supported Linux GPUs. Label undocumented visual behavior unknown. | Needs evidence-based contract |
| P0-07 | Undo and redo | Which project edits are atomic operations in this implementation, and what state must persist after save/reopen? | Define operation boundaries; verify undo/redo and persistence with state snapshots for hierarchy, properties, and scripts. | Needs evidence-based contract |
| P0-08 | Run and stop a basic local playtest | Which runtime APIs and state-isolation guarantees are documented and implemented? | Test deterministic local experiences against explicit service/runtime contracts; verify stopping playtest leaves saved editor state unchanged. | Needs evidence-based contract |

## Evidence rules

- Keep direct observations separate from interpretation, inference, hypothesis, and unknowns.
- For static analysis, start with an installation inventory, then select one binary and one behavior question. Follow instructions, callers/callees, strings, constants, and references; pseudocode alone is not sufficient evidence.
- Use REA evidence only after its provider and target provenance are recorded. Include the REA/provider version, target path, Studio version, and SHA-256 of every analyzed binary.
- A passing Linux contract or fixture test does not establish parity with Windows Studio. Record its scope and avoid making claims beyond the evidence available.
