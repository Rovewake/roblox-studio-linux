# ADR-0001: Native Linux stack candidates

**Status:** Rust/core and WGPU remain selected for the first native prototype; egui is not the final Studio UI decision and must be reconsidered before Phase 4.
**Date:** 2026-10-28.

## Context

The product must be a native Linux desktop editor with a substantial hierarchy, long code files, a GPU viewport, local project formats, and Luau integration. It must support Wayland and X11, remain portable across Linux distributions, avoid author-specific assumptions, and allow later optimizations to be measured against a reproducible baseline. There is no existing product code or team investment that requires a particular language.

The current host is NixOS 26.05 x86_64 with an Intel `i915` GPU in a Niri/Wayland session and Xwayland. The environment audit and available comparisons are in the [Phase 0 baseline](../research/phase-0-baseline.md). The spike under `prototypes/phase-0-native-spike/` is an isolated candidate, not yet a product commitment.

The final editor needs to reproduce a specific Studio version's layout, docking, panels, commands, editor behavior, and keyboard interactions. Public Roblox engineering posts describe the older core Studio UI as Qt/C++/QML-heavy, the newer UI as Luau-based, some remaining Qt docking/window infrastructure in 2025, and a continued migration of Qt panels in September 2026. The selected WindowsStudio64 binary has not yet yielded usable REA findings for this component split, so a fidelity-first UI framework choice remains open. The version-pinned UI investigation is a required input to Phase 4.

## Candidates compared

| Area | Candidate | Strengths | Risks / questions | Phase 0 decision |
|---|---|---|---|---|
| Language | Rust | Fits the Rust `rbx-dom` ecosystem; memory safety; static native binaries; direct C interop for Luau | Learning and build complexity; UI ecosystem needs real input-method/accessibility evaluation | Accept for the initial native implementation; maintain subsystem interfaces and revisit if implementation evidence contradicts the choice |
| Language | C++ | Mature native GUI/rendering ecosystem; strong direct API control | Manual ownership complexity; no existing code; format/runtime integration requires more bindings | Alternative; no technical evidence currently outweighs Rust for this project |
| Language | C#/.NET | Productive editor/runtime tooling; mature desktop application support | Native graphics/data integration and Linux deployment add runtime/packaging choices | Not selected for Phase 0 spike; revisit only if Rust/GUI results fail creator needs |
| UI and window | Rust `eframe`/egui + `winit` + `egui-wgpu` | One Rust path for immediate UI and WGPU viewport; explicit Wayland/X11 backends; custom renderer callbacks | egui is not native-looking; accessibility, IME, docking, clipboard, and large text behavior still need real desktop and creator evaluation; no binary-derived Studio UI map yet | Accepted only for the Phase 0 stress prototype; do not treat as the final Studio shell choice |
| UI | GTK4 + gtk-rs | Native Linux widgets, desktop integration, Wayland/X11 support | Custom WGPU scene embedding needs a clear interop boundary; GTK renderer is not the scene renderer | Viable alternative if editor-native behavior outweighs single-renderer integration |
| UI | Qt 6 | Mature desktop widgets, platform plugins, QML, and graphics integration; Qt 6.12 lists specific supported Linux desktop configurations and Windows | Larger runtime/toolchain and C++/Rust boundary if Rust owns the data/runtime core; NixOS and other target-distro packaging need validation; cross-platform Qt does not provide Roblox's unavailable source or make Windows Qt binaries portable | Strong candidate for the native Studio UI; evaluate after REA identifies the selected version's Qt/QML, Luau UI, and OS-specific boundaries |
| Renderer | `wgpu` with Vulkan preferred | Safe Rust API; explicit GPU adapter selection; supports custom rendering inside egui | Adapter/driver feature differences; GPU timestamps are optional; physical GPU behavior remains untested in this phase | Accept WGPU as the renderer abstraction for the first slice. Use Vulkan for the headless spike; hardware driver support remains unqualified |
| Renderer | OpenGL / `glow` | Broad hardware availability; simpler fallback on some Linux systems | Legacy API model; independent renderer path if egui uses WGPU; driver behavior still varies | Keep as a measured fallback, not an untested automatic path |
| Renderer | Direct Vulkan | Full control over Linux driver behavior and explicit synchronization | High implementation and maintenance cost before engine semantics are known | Defer until a profile identifies a need WGPU cannot meet |
| Project storage | `rbx-dom` + project-owned domain model | Existing Rust serializers for Roblox XML/binary formats; public compatibility notes and test cases | Coverage gaps, version drift, and third-party license/round-trip review are required | Phase 2 candidate only; do not let serializer types define the editor model without review |
| Project storage | Fully custom serializer/domain | Maximum control over unknown-field preservation and compatibility policy | Large format implementation and maintenance burden | Keep only if `rbx-dom` coverage fails required fixtures |
| Luau | Official Luau VM/C API behind a Rust runtime module | Uses the language implementation and C API; can keep language runtime separate from engine services | Requires explicit engine model, scheduler, bindings, and compatibility layer | Select as the Phase 3 language integration direction; do not claim full Roblox runtime coverage |
| Packaging | Flatpak | Reproducible sandboxed distribution and desktop integration | GPU access, filesystem portals, permissions, and distribution-specific behavior need verification | Defer selection until Linux support matrix and file-dialog behavior are tested |
| Packaging | Distribution packages / AppImage | Integrates with distribution package manager or offers a simple single-file option | Multiple dependency targets, update behavior, rollback, and desktop integration differ | Keep as alternatives; choose from measured distro coverage and maintenance cost |
| Plugin system | In-process extension API or out-of-process host | Both can support extensions with different capability and isolation boundaries | No P0 plugin requirement; early choice could constrain the model and security boundary | Defer. Establish stable project/runtime interfaces and plugin workflows first |

## Phase 0 measurement and decision

The spike pins Rust `1.99.0`, `eframe`/egui `0.37.0`, `winit` `0.30.13`, and WGPU `30.0.1` through `Cargo.lock`. It exercises 10,000 hierarchy items, a 20,000-line editable source buffer, and 2,000 instanced primitives. Five 30-second captures per backend passed in private Weston headless (Wayland) and Xvfb (X11) displays; both selected Vulkan llvmpipe on Mesa 26.1.8. The raw values, across-run ranges, and interpretation limits are in the [Phase 0 result summary](../../benchmarks/results/README.md). These are software-rendering diagnostics and not desktop, GPU, or product SLO results.

Use Rust for the prototype core, eframe/egui for the Phase 0 stress UI, winit for Wayland/X11 windowing, and wgpu via egui-wgpu for the renderer. Keep window/event handling, UI, renderer, project model/serializer, Luau VM, and desktop integration behind separate interfaces. Before the Phase 4 shell, use REA evidence from the pinned Studio build to revisit Qt 6 versus egui and record the product decision in a new ADR. Hardware GPU fallback, storage library adoption, packaging, and plugin architecture remain open.

## Consequences

- Phase 0 tested the candidate UI/render path in isolated headless servers without taking focus from the user's desktop.
- CI can build and lint this small candidate while the project model remains undecided.
- The spike must not introduce author-specific paths, persistent configuration, or assumptions about one GPU.
- Phase 2 must compare `rbx-dom` against fixtures and inspect its license and missing-format coverage before adoption.
- Phase 4 must re-evaluate editor input methods, accessibility, keyboard behavior, docking, native file dialogs, and portability before the UI choice is considered stable. Physical GPU and desktop-compositor validation is also required before any environment is advertised as supported.

## Primary sources

- [winit 0.30.13 documentation](https://docs.rs/winit/0.30.13/winit/) and [Linux backend features](https://docs.rs/crate/winit/0.30.13/features).
- [eframe 0.37.0 documentation](https://docs.rs/eframe/0.37.0/eframe/) and [egui-wgpu custom rendering callback](https://docs.rs/egui-wgpu/0.37.0/egui_wgpu/trait.CallbackTrait.html).
- [wgpu project](https://github.com/gfx-rs/wgpu) and [WGPU backend selection](https://docs.rs/wgpu/30.0.1/wgpu/enum.Backend.html).
- [GTK4 Linux build/backends](https://docs.gtk.org/gtk4/building.html) and [GTK4 rendering overview](https://docs.gtk.org/gtk4/overview.html).
- [Qt Linux requirements](https://doc.qt.io/qt-6/linux-requirements.html) and [Qt platform abstraction](https://doc.qt.io/qt-6/qpa.html).
- [Qt 6.12 supported platforms](https://doc.qt.io/qt-6/supported-platforms.html) — lists specific desktop Linux and Windows configurations; the selected distribution and packaging still need validation.
- [Roblox engineering: Next Gen Studio UI](https://devforum.roblox.com/t/next-gen-studio-ui-preview-is-here-beta/3075390/1/) and [later engineering reply on Qt docking](https://devforum.roblox.com/t/next-gen-studio-ui-preview-is-here-beta/3075390?page=101) — show a transition from Qt/C++ toward Luau UI while some Qt window/docking infrastructure remained.
- [Roblox engineer's account of the earlier core UI](https://devforum.roblox.com/t/what-gui-library-was-used-for-initial-development/3489839) — says most of the pre-Next Gen Studio core UI was C++ Qt and QML, with some Luau.
- [September 2026 Studio 738 release notes discussion](https://devforum.roblox.com/t/release-notes-for-738/4861725/1) — a Roblox staff member says the remaining Qt Studio UI panels and dialogs are intended to migrate to Luau.
- [Luau C API](https://luau.org/api/) and [XDG Base Directory specification](https://specifications.freedesktop.org/basedir/latest/).
- [rbx-dom](https://github.com/rojo-rbx/rbx-dom), subject to Phase 2 coverage and license review.
