# Phase 1 P0 workflow investigation plan

**Status:** planned, not observed. The original Windows Studio installation and a supported REA analysis host are not currently accessible; see [the REA lab audit](phase-1-rea-lab.md).

The workflows below come from the project's P0 scope. Their descriptions are investigation questions, not claims about current Studio behavior. For each one, record the original Studio version, project fixture/hash, system/GPU/driver, exact steps, visible result, confidence, and any REA evidence used. Use a disposable local project and do not include account credentials or private project data.

| ID | Workflow | First question for the reference Studio | Evidence and differential check to plan | Status |
|---|---|---|---|---|
| P0-01 | Launch and open a local project | What is the normal startup sequence, and what state is visible after a small local project finishes opening? | Record startup/open steps and timing; inventory the install directory; compare the opened project and visible state with the Linux implementation. | Awaiting reference access |
| P0-02 | Create, save, close, and reopen a local project | Which local project formats and save paths are offered, and what survives a close/reopen round trip? | Use a disposable project with a small known hierarchy and script; hash source/outputs and compare semantic contents after reopening. | Awaiting reference access |
| P0-03 | Navigate the hierarchy | How do selection, expansion, ordering, and common child operations affect the visible tree and project state? | Record tree before/after each action and relevant undo behavior; compare the resulting instance graph and selection state. | Awaiting reference access |
| P0-04 | Edit common properties | Which common properties can be changed, how are invalid values handled, and what persists after save/reopen? | Record property type/value, validation feedback, serialized result, and undo/redo outcome; compare supported values and data after round trip. | Awaiting reference access |
| P0-05 | Edit, analyze, and save Luau scripts | What editor feedback appears for valid and invalid scripts, and what script text persists after save/reopen? | Use a disposable script fixture; record diagnostics and exact text changes; compare saved source and analysis results where an equivalent implementation exists. | Awaiting reference access |
| P0-06 | Use the interactive viewport | How do camera navigation, selection, and common transforms affect the visible scene and corresponding project data? | Use a small deterministic scene; record each action and before/after transforms; compare scene state and viewport behavior. | Awaiting reference access |
| P0-07 | Undo and redo | What is the unit of an undoable action across hierarchy, property, and script edits, and what remains after save/reopen? | Perform one action from each supported edit category, undo/redo it, then save/reopen and compare exact state. | Awaiting reference access |
| P0-08 | Run and stop a basic local playtest | Which local changes are isolated to the simulation, and what state remains in the saved project after stopping playtest? | Use a disposable local experience; record start/stop output and project state before/after; verify no saved data changes without an explicit editor action. | Awaiting reference access |

## Evidence rules

- Keep direct observations separate from interpretation, inference, hypothesis, and unknowns.
- For static analysis, start with an installation inventory, then select one binary and one behavior question. Follow instructions, callers/callees, strings, constants, and references; pseudocode alone is not sufficient evidence.
- Use REA evidence only after its provider and target provenance are recorded. Include the REA/provider version, target path, Studio version, and SHA-256 of every analyzed binary.
- A comparison test is only planned until both the original Studio and a corresponding Linux implementation can be run with the same disposable data.
