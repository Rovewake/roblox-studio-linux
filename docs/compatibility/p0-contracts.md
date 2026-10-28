# P0 workflow compatibility contracts

**Status:** contracts specified; native implementation and acceptance checks pending.

**Evidence reviewed:** 2026-10-28 technical documentation and public format references.

**Studio reference:** WindowsStudio64 `0.745.0.7451137`, upload ID `version-7062aec9dbe74b14`; provenance and SHA-256 values are in the [Phase 1 REA lab audit](../research/phase-1-rea-lab.md).

### Shared static-analysis provenance

| Item | Recorded value |
|---|---|
| Host | NixOS 26.05, x86_64 |
| REA | `rea-agents-6.8.0` |
| Native provider | Ghidra 12.1.4; JDK 21.0.12.1+1, both temporary |
| Target | `RobloxStudioBeta.exe`, SHA-256 `781b36ed6fb62e61292b9e6a64327280c7da12b00f3ea8b2c91a3f4834f860ec` |
| Attempted REA query | `rea search ... --kind strings --provider ghidra`, searching for `.rbxl` |
| Result | Timed out at 330 seconds and again at 900 seconds; no usable finding. None of the contracts below is inferred from that failed search. |

The acquisition manifest, archive hash, ETag, complete command context, provider check, and package inventory limits are recorded in the [REA lab audit](../research/phase-1-rea-lab.md). The web documentation and public references linked below were checked on the evidence-review date above; revalidate them when implementing a later phase.

## Scope and evidence limits

This document defines the first Linux implementation contracts for the eight P0 workflows. It is a project specification, not a report of observed behavior in a running Roblox Studio session. The reference package was acquired and inventoried read-only; it was not executed. This project has no Windows test environment, and no contract below requires Windows testing or claims direct parity with Windows Studio.

The sources establish selected public facts about Roblox place files, data model concepts, Luau, and Studio workflows. Where those sources do not define behavior, this document states an explicit product contract or marks the matter unknown. `rbx-dom` and `RobloxAPI/spec` are independent community references, not authoritative specifications of Studio behavior. An independent parser accepting a file does not prove Studio accepts it.

### Confidence labels

- **Documented fact — high:** directly stated in the linked technical source or project documentation.
- **Format cross-check — medium:** independently implemented or community-documented behavior; useful for testing, but not authoritative.
- **Linux contract — proposed:** behavior this project will implement and test. It is not a claim that Studio behaves identically.
- **Unknown:** available evidence does not settle the behavior. Keep it unknown until a documented source, reproducible fixture, static evidence, or Linux creator feedback resolves it.

## Shared contract rules

1. The app works on user-selected local project paths without silently moving or overwriting project data.
2. Unsupported or malformed data must be reported clearly. The app must preserve unmodified unknown data where it can; if it cannot safely preserve it, it must refuse a destructive save and explain why.
3. UI selection and edits operate on stable in-memory instance identities, never on a name or display path alone.
4. P0 acceptance fixtures are deterministic, disposable, versioned, and contain no credentials or private creator data.
5. Each check below is planned, not yet run. It becomes an executable Linux acceptance test as the corresponding subsystem is implemented.
6. Linux behavior and performance claims are bounded by the named fixture, build, host, GPU, driver, graphics backend, and desktop session. No Windows comparison is implied.

## P0-01 — Launch and open a local project

### Evidence

- Roblox's [Studio CLI documentation](https://create.roblox.com/docs/studio/cli) documents opening a local `.rbxl` or `.rbxlx` place with `--task EditFile --localPlaceFile <path>`, or by passing the path as the first argument. This documents the reference application's interface; its listed installation paths are Windows and macOS and do not establish a Linux Studio build.
- The [place file documentation](https://create.roblox.com/docs/projects/place-files) identifies `.rbxl` as the binary place format and `.rbxlx` as the XML-based place format.

### Linux contract

- Starting without a project presents a local start/open workflow and does not require account sign-in or network access to open a local file.
- Opening a supported local place loads its data model and reports the selected path. Paths with spaces and non-ASCII characters are supported.
- A missing, unreadable, malformed, or unsupported file produces an actionable error. A failed open does not alter the source file or silently replace the currently open project.

### Planned Linux fixture/check

Use a clean temporary user profile and disposable valid `.rbxlx` and `.rbxl` fixtures. Open each by path, including paths containing spaces and non-ASCII characters. Also try a missing path, a permission-denied file where the host permits that fixture, a truncated file, and an unsupported extension. Verify the loaded project identity, actionable error text, and SHA-256 equality of every input before and after failed opens. Repeat with a cold start and a second project already open.

### Confidence and unknowns

- **Documented fact:** high for the reference CLI's local file arguments and the two file extensions.
- **Linux contract:** proposed; acceptance pending.
- **Unknown:** exact Studio startup screen, error wording, recent-file behavior, recovery prompts, and any undocumented startup behavior. None is asserted here.

## P0-02 — Create, save, close, and reopen a local project

### Evidence

- Roblox documents local export and the `.rbxl` binary / `.rbxlx` XML distinction in [Place files](https://create.roblox.com/docs/projects/place-files).
- The community-maintained [RBXL binary format specification](https://github.com/RobloxAPI/spec/blob/master/formats/rbxl.md) and [RBXLX XML format specification](https://github.com/RobloxAPI/spec/blob/master/formats/rbxlx.md) describe the formats based on independent analysis. Treat them as useful format references, not Roblox-published guarantees.
- [rbx-dom](https://github.com/rojo-rbx/rbx-dom) is an independent Rust implementation that reads and writes Roblox XML and binary model/place formats. The community [format test-file collection](https://github.com/RobloxAPI/testfiles) and rbx-dom's [compatibility notes](https://dom.rojo.space/compatibility.html) provide cross-checks and document coverage limits; neither is an oracle for Studio acceptance.

### Linux contract

- New projects are created in an explicit user-selected location; existing projects are never silently relocated.
- The user can save to a chosen supported format and reopen it with semantic equality for all declared supported fields: hierarchy, class, property values, scripts, attributes, tags, and supported references.
- Saves use a safe replace strategy so a failed write leaves the previous project file intact.
- Unknown fields are preserved byte-for-byte or semantically where feasible. If preservation cannot be guaranteed for a file the user is editing, saving that file is refused with a clear explanation rather than silently dropping data.
- The UI distinguishes unsaved edits from the last successfully written file.

### Planned Linux fixture/check

Create fixtures in each supported format with nested instances, duplicate display names, scripts, common property types, attributes, tags, references, and unknown fields drawn from public format fixtures. Parse → edit one supported field → serialize → parse again and compare the semantic model. Cross-check overlapping cases with a pinned `rbx-dom` version and public format fixtures. Inject a write failure (for example, an unwritable destination or a controlled failing file adapter) and verify the prior file hash is unchanged. Record serializer versions, fixture hashes, unsupported elements, and every known round-trip gap.

### Confidence and unknowns

- **Documented fact:** high for the two extensions and their broad format distinction.
- **Format cross-check:** medium; public specifications and `rbx-dom` have independent coverage and gaps.
- **Linux contract:** proposed; acceptance pending.
- **Unknown:** full format coverage and how each Studio version handles every unknown tag, property, reference, or binary chunk. Exact Studio save prompts and default format choices are not specified here.

## P0-03 — Navigate and edit the hierarchy

### Evidence

- The [Explorer documentation](https://create.roblox.com/docs/studio/explorer) describes the instance tree, parent-child branches, reparenting, insertion, duplication, and selection-related behavior.
- The [DataModel API reference](https://create.roblox.com/docs/reference/engine/classes/DataModel) describes the root data model and its service-facing API; the [Instance API reference](https://create.roblox.com/docs/reference/engine/classes/Instance) exposes `Name`, `Parent`, and child/descendant operations.
- [Studio interface documentation](https://create.roblox.com/docs/studio/overview) describes Explorer as a view of the place data model and Workspace as the content represented in the viewport.

### Linux contract

- The hierarchy view reflects the project's parent-child graph. Selecting an item in the hierarchy selects that same instance in the property inspector and, when renderable, the viewport.
- Reparent, insert, duplicate, and delete operations update the single shared project model and create undoable edits.
- Instance identity is stable across rename and reparent operations. `Name` and displayed ancestry are labels, not internal identity keys. Duplicate names are supported by the model and must remain distinguishable in the UI.
- Unsupported nodes remain visible as data even when the viewport cannot render them.

### Planned Linux fixture/check

Build a fixture containing a root, nested parents, two same-class siblings with the same name, a supported part, a script, and an unsupported node. Assert parent-child membership independent of UI sort order; select each duplicate by stable identity; rename and reparent one; insert, duplicate, and delete nodes; verify all panes reference the intended identity; then undo/redo and save/reopen. Add depth and node-count cases to establish safe limits and measure tree refresh latency as the implementation grows.

### Confidence and unknowns

- **Documented fact:** high for the existence of the hierarchical model and parent/child operations.
- **Linux contract:** proposed; acceptance pending.
- **Unknown:** exact default ordering, keyboard bindings on Linux, selection persistence, context menu content, and visual treatment of hidden services. These are implementation and creator-feedback questions, not inferred Studio facts.

## P0-04 — Inspect and edit common properties

### Evidence

- The [Properties window documentation](https://create.roblox.com/docs/studio/properties) says the selected object's class and name appear in the header, properties are grouped, and tags and attributes can be managed there. It lists attribute value types and says attributes and tags are serialized with saved places/assets.
- The [Instance API reference](https://create.roblox.com/docs/reference/engine/classes/Instance) provides the base instance properties and methods. Class-specific properties should be sourced from the relevant versioned technical API reference, not guessed from UI labels.

### Linux contract

- The inspector follows the stable selected instance identity and displays its class and name.
- Initial editable property support is explicit and typed. For each supported class/property pair, metadata defines type, validation, default representation, serialization mapping, and whether the field is editable.
- Invalid or read-only edits are rejected without changing project state. Valid edits update the shared model, become undoable, and persist on save/reopen.
- Supported attributes and tags are represented as data and round-trip through project files. Unknown properties remain preserved even if not editable.

### Planned Linux fixture/check

For each initially supported class, generate fixtures for default values, ordinary values, boundary values, enums, vectors/transforms, optional values, attributes, and tags. Verify inspector selection, valid commit, invalid input rejection, undo/redo, semantic serialization, and reopen. Include two same-name instances to ensure selection does not resolve by name. Compare property metadata with a pinned official API metadata version and record the version used.

### Confidence and unknowns

- **Documented fact:** high for selection-dependent inspector structure and listed attribute/tag persistence.
- **Linux contract:** proposed; acceptance pending.
- **Unknown:** exact grouping, formatting, validation messages, hidden properties, editor defaults, and behavior of unsupported class/property combinations in each Studio release.

## P0-05 — Edit, analyze, and save Luau scripts

### Evidence

- The [Studio Script Editor documentation](https://create.roblox.com/docs/studio/script-editor) describes syntax highlighting, completion, declaration navigation, search/replace, script analysis, and multi-cursor editing. It says a multi-cursor edit counts as one undo/redo action across cursors.
- The [Luau project](https://github.com/luau-lang/luau) and its [getting-started guide](https://luau.org/getting-started/) provide the language implementation and command-line tools, including `luau` and `luau-analyze`.
- Luau's [type documentation](https://luau.org/types/) defines `--!nocheck`, `--!nonstrict`, and `--!strict` modes. Its [lint documentation](https://luau.org/lint/) catalogs analyzer lint rules.

### Linux contract

- Editing preserves the exact source text through save/reopen, including source that has syntax or type errors. Diagnostics do not block saving text.
- Syntax/compiler errors, type diagnostics, lint warnings, and playtest Output are separate categories with source locations where the tool provides them.
- The bundled or selected Luau toolchain is version-pinned and recorded. The app honors the file's type mode and does not silently rewrite code.
- Roblox engine API completion and type information are exposed only from versioned metadata the project can identify. Missing engine symbols are reported as unsupported or unknown; they are not invented from core Luau behavior.

### Planned Linux fixture/check

Use a pinned Luau build and a corpus containing valid code, syntax errors, strict and non-strict type mismatches, `--!nocheck`, lint warnings, Unicode, line endings, and multi-cursor edits. Verify stable source round-trip, diagnostic categories and locations, mode handling, and that errors do not prevent save. Verify one multi-cursor edit is one undo unit. Record toolchain version and expected analyzer output; keep Roblox engine API tests separate from language tests.

### Confidence and unknowns

- **Documented fact:** high for Luau syntax/type/lint capabilities and documented Script Editor features.
- **Linux contract:** proposed; acceptance pending.
- **Unknown:** exact Studio analyzer version/configuration, engine reflection/autocomplete data version, diagnostic scheduling, and many editor-specific behaviors. Do not claim those match Studio until evidence supports it.

## P0-06 — Use the interactive viewport

### Evidence

- The [Studio interface reference](https://create.roblox.com/docs/studio/overview) describes the 3D viewport as a view of Workspace and documents camera motion, focus, zoom, pan, object hover/selection, modifier selection, and selection cycling for occluded objects.
- The [Parts guide](https://create.roblox.com/docs/parts) documents primitive parts and their position, size, orientation, and color concepts.
- The [CFrame API](https://create.roblox.com/docs/reference/engine/datatypes/CFrame) defines Roblox transforms, including position/orientation and object/world-space conversion. The [Pivot tools guide](https://create.roblox.com/docs/studio/pivot-tools) and [Camera API](https://create.roblox.com/docs/reference/engine/classes/Camera) are references for future transform and camera details.

### Linux contract

- The initial viewport renders supported primitive Workspace geometry with Roblox coordinate and transform semantics. Unsupported meshes, terrain, materials, and effects are identified as unsupported and do not corrupt their underlying data.
- The user can orbit, pan, zoom, focus the selected object, select a visible object, and use a deterministic selection cycle for overlapping supported objects.
- Move, rotate, and scale tools modify the same data model shown by the inspector. World/local axes and snap increments are explicit settings; the exact defaults are project choices until creator feedback establishes usable defaults.
- A committed transform is undoable and persists through save/reopen.

### Planned Linux fixture/check

Use generated Parts with known positions, orientations, sizes, and pivots. Check CFrame composition/inversion against documented API examples; test camera projection and picking rays against expected geometry; verify nearest visible hit and repeated selection-cycle results; exercise move/rotate/scale in world and local axes at configured snap values; compare model transforms before/after save/reopen. Measure frame time (median and P95), CPU, and GPU on each declared supported Linux GPU/driver/session using the same fixture and scripted camera path. Report limits and noise; do not claim performance relative to Windows Studio.

### Confidence and unknowns

- **Documented fact:** high for the coordinate API and publicly documented camera/selection interactions.
- **Linux contract:** proposed; acceptance pending.
- **Unknown:** renderer implementation, GPU backend behavior, exact hit-testing rules for every object type, gizmo mathematics, snap defaults, undocumented camera settings, and Linux-specific key bindings.

## P0-07 — Undo and redo

### Evidence

- The [ChangeHistoryService API](https://create.roblox.com/docs/reference/engine/classes/ChangeHistoryService) provides recording, commit/finish, undo, and redo operations for plugin-originated modifications. This API does not define the native editor's complete history policy.
- The [Script Editor documentation](https://create.roblox.com/docs/studio/script-editor) explicitly says edits made across multiple cursors count as one undo/redo action.

### Linux contract

- Each committed semantic edit command is one history entry: a hierarchy command, a committed property edit, a transform gesture, or a script edit transaction. Multi-cursor edits are one atomic entry.
- Undo restores the prior project model; redo reapplies it. A new edit after undo clears the redo branch. No-op commands create no history entry.
- Playtest-only state is outside editor history. Saving writes the current model but does not prevent subsequent undo; undo after save marks the in-memory project dirty and does not rewrite the file until another explicit save.
- Ordinary keystroke coalescing and history retention limits are implementation choices to be measured and refined from creator feedback.

### Planned Linux fixture/check

Perform one insert, reparent, property commit, transform gesture, script transaction, and multi-cursor edit. Snapshot the model before and after each, undo once, and compare to the exact prior snapshot; redo and compare to the committed snapshot. Undo, make a different edit, and verify redo is unavailable. Verify no-op behavior, save then undo (disk hash unchanged until explicit save), and ensure a playtest mutation creates no editor undo item.

### Confidence and unknowns

- **Documented fact:** high for plugin recording APIs and the multi-cursor rule.
- **Linux contract:** proposed; acceptance pending.
- **Unknown:** Studio's native undo grouping for typing, property edits, transforms, save boundaries, and history limits. The plugin API is not evidence for those native editor choices.

## P0-08 — Run and stop a basic local playtest

### Evidence

- [Studio testing modes documentation](https://create.roblox.com/docs/studio/testing-modes) describes `Run` as simulating without inserting an avatar and `Stop` as ending simulation and resetting objects/instances to their pre-playtest state. `Test`, `Test Here`, client/server toggling, and other modes are documented separately.

### Linux contract

- P0 includes one local `Run` mode without an avatar and a `Stop` action. Avatar, multi-client, server/client, and network simulation modes are outside this initial P0 contract.
- Runtime execution uses a disposable snapshot of the editor model. Runtime-only object/property changes are discarded on Stop; the editor model and saved project remain unchanged unless the user explicitly performs an editor save.
- Runtime `print`, `warn`, and errors appear in a playtest Output view, separate from static Luau diagnostics.
- If a script uses an unsupported runtime API, the app reports that limitation with a source location where available; it does not silently substitute unrelated behavior.

### Planned Linux fixture/check

Run a local fixture whose script prints a known message, changes a property, creates an instance, and then raises a controlled error. Verify output classification and source location, Stop the run, and compare the editor-model snapshot and saved-file SHA-256 with the pre-run values. Repeat Run/Stop, including a simulated runtime crash, and ensure runtime state is released. Confirm Run does not require network access for this fixture.

### Confidence and unknowns

- **Documented fact:** high for reference Studio's Run-without-avatar and Stop/reset description.
- **Linux contract:** proposed; acceptance pending.
- **Unknown:** exact scheduler, physics, service/API coverage, error formatting, state restoration details for edge cases, and the limits of an eventual local runtime. Full Roblox engine simulation is not implied by this contract.

## P0 exit-gate coverage

The table below links each P0 scope item to its supporting source family and the future Linux check. At Phase 1, this means a reproducible acceptance procedure is specified; it does not mean an application or test has already run.

| ID | Evidence family | Acceptance check specified | Linux implementation/test status |
|---|---|---|---|
| P0-01 | Studio CLI and place-file docs | Clean-profile open/error/hash fixtures | Pending |
| P0-02 | Place-file docs, public format references, `rbx-dom` | Semantic round-trip, failure safety, cross-check | Pending |
| P0-03 | Explorer, DataModel, Instance docs | Identity, duplicate names, tree mutations, persistence | Pending |
| P0-04 | Properties and Instance docs, API metadata | Typed property/attribute validation and persistence | Pending |
| P0-05 | Studio Script Editor, Luau compiler/analyzer docs | Source round-trip, modes, diagnostic corpus | Pending |
| P0-06 | Studio viewport docs, Parts, CFrame, Camera | Deterministic transform/picking plus Linux frame-time profile | Pending |
| P0-07 | ChangeHistoryService and Script Editor docs | Snapshot-based command undo/redo | Pending |
| P0-08 | Studio testing modes | Runtime snapshot isolation and Stop-state fixture | Pending |

## References

### Roblox technical documentation

- [Studio CLI](https://create.roblox.com/docs/studio/cli)
- [Place files](https://create.roblox.com/docs/projects/place-files)
- [Explorer window](https://create.roblox.com/docs/studio/explorer)
- [Properties window](https://create.roblox.com/docs/studio/properties)
- [Instance API](https://create.roblox.com/docs/reference/engine/classes/Instance)
- [DataModel API](https://create.roblox.com/docs/reference/engine/classes/DataModel)
- [Studio interface and viewport](https://create.roblox.com/docs/studio/overview)
- [Parts](https://create.roblox.com/docs/parts)
- [CFrame](https://create.roblox.com/docs/reference/engine/datatypes/CFrame)
- [Camera API](https://create.roblox.com/docs/reference/engine/classes/Camera)
- [Pivot tools](https://create.roblox.com/docs/studio/pivot-tools)
- [Script Editor](https://create.roblox.com/docs/studio/script-editor)
- [ChangeHistoryService API](https://create.roblox.com/docs/reference/engine/classes/ChangeHistoryService)
- [Testing modes](https://create.roblox.com/docs/studio/testing-modes)

### Luau and independent format references

- [Luau source repository](https://github.com/luau-lang/luau)
- [Luau getting started](https://luau.org/getting-started/)
- [Luau type system](https://luau.org/types/)
- [Luau lint rules](https://luau.org/lint/)
- [RBXL binary format specification](https://github.com/RobloxAPI/spec/blob/master/formats/rbxl.md) and [RBXLX XML format specification](https://github.com/RobloxAPI/spec/blob/master/formats/rbxlx.md) — community-maintained, independently derived references.
- [RobloxAPI format test files](https://github.com/RobloxAPI/testfiles) — community fixtures for independent parser/serializer checks.
- [rbx-dom](https://github.com/rojo-rbx/rbx-dom) and [compatibility notes](https://dom.rojo.space/compatibility.html) — independent serializer/parser and documented compatibility limits.
