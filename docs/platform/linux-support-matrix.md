# Linux support matrix

**Status:** validation targets selected; no product environment is supported or release-qualified yet.
**Last updated:** 2026-10-28.

The project is Linux-only and targets x86_64 first. A listed target is a test plan until the application is installed and its announced workflows pass there. A GitHub build runner does not establish GPU, driver, compositor, or desktop integration support.

## Current development host

| Host label | Distribution/kernel | CPU/memory | GPU/driver | Session | Evidence status |
|---|---|---|---|---|---|
| `dev-nixos-intel` | NixOS 26.05; Linux 6.18.57; x86_64 | Intel Core i3-1215U, 6 cores/8 threads; 11 GiB RAM; no swap | Intel PCI `8086:46B3`; kernel `i915`; `/dev/dri/renderD128` accessible; Mesa 26.1.8 | Niri on Wayland with Xwayland | Host inventory complete. Headless tests selected llvmpipe; physical interactive session is not release-qualified. |

The host inventory used read-only system queries and is documented in [the Phase 0 baseline](../research/phase-0-baseline.md). Hardware identifiers describe the measurement environment; they are not product defaults. Automated graphical tests use isolated headless servers and never focus, resize, or switch windows on the user's interactive desktop. Raw measurements and their limits are documented in [the Phase 0 headless results](../../benchmarks/results/README.md).

## Initial validation matrix

| ID | Distribution / session | Architecture | GPU/backend | Role | Status |
|---|---|---|---|---|---|
| LNX-HDL-01 | NixOS 26.05, private Weston 16 headless Wayland | x86_64 | Vulkan llvmpipe; Mesa 26.1.8; LLVM 21.1.8 | Isolated Wayland client/render checks and Phase 0 software baseline | Five 30-second runs completed; no physical GPU or desktop support claim |
| LNX-HDL-02 | NixOS 26.05, private Xvfb 21.1.24 X11 | x86_64 | Vulkan llvmpipe; Mesa 26.1.8; LLVM 21.1.8 | Isolated X11 client/render checks and Phase 0 software baseline | Five 30-second runs completed; not Xwayland or native Xorg |
| LNX-DEV-01 | NixOS 26.05, Niri Wayland | x86_64 | Intel `i915`; Vulkan preferred, GLES only as a separately recorded fallback | Interactive development and future physical-driver validation | Not release-qualified; automated tests must not take focus from the user's active desktop |
| LNX-DEV-02 | NixOS 26.05, Xwayland under Niri | x86_64 | Same Intel GPU and Vulkan driver | Exercise the X11 window backend in the available host session | Not release-qualified; not equivalent to native Xorg |
| LNX-CI-01 | Ubuntu 26.04 LTS hosted runner, headless | x86_64 | Software Vulkan; no GPU guarantee | CI build, static checks, unit tests, and virtual-display renderer smokes | Workflow added; confirm on GitHub Actions before marking passed |
| LNX-DIST-01 | Ubuntu 26.04 LTS desktop, GNOME Wayland | x86_64 | Intel or AMD Vulkan hardware | First broad desktop distribution validation before release claims | Not yet available/tested |
| LNX-DIST-02 | Ubuntu 26.04 LTS desktop, Xorg session | x86_64 | Intel or AMD Vulkan hardware | Native X11 desktop validation | Not yet available/tested |
| LNX-GPU-AMD | A supported Linux desktop from LNX-DIST-01/02 | x86_64 | AMD Radeon with Mesa RADV Vulkan | GPU/driver coverage | Not yet available/tested |
| LNX-GPU-NVIDIA | A supported Linux desktop from LNX-DIST-01/02 | x86_64 | NVIDIA proprietary Vulkan driver | GPU/driver coverage | Not yet available/tested |

Ubuntu 26.04 LTS is selected as the first reproducible distribution target because it is the current LTS release, with standard security maintenance through May 2031. Its GitHub hosted runner image is available for compile/static CI; it does not replace an interactive machine. See [Ubuntu's official release lifecycle](https://ubuntu.com/about/release-cycle) and the [runner image inventory](https://github.com/actions/runner-images/tree/main/images/ubuntu).

## User data and configuration paths

When implemented, use the XDG Base Directory defaults and respect valid environment overrides:

| Data | Path |
|---|---|
| Configuration | `$XDG_CONFIG_HOME/roblox-studio-linux/` (default `~/.config/roblox-studio-linux/`) |
| User state and logs | `$XDG_STATE_HOME/roblox-studio-linux/` (default `~/.local/state/roblox-studio-linux/`) |
| App-managed durable data | `$XDG_DATA_HOME/roblox-studio-linux/` (default `~/.local/share/roblox-studio-linux/`) |
| Rebuildable cache | `$XDG_CACHE_HOME/roblox-studio-linux/` (default `~/.cache/roblox-studio-linux/`) |
| Runtime sockets/locks | `$XDG_RUNTIME_DIR/roblox-studio-linux/` when needed |
| Creator projects | Explicit paths chosen by the creator; never silently relocated into app data |

These are implementation contracts, not paths written by the Phase 0 spike. The [freedesktop.org XDG Base Directory specification](https://specifications.freedesktop.org/basedir/latest/) defines the environment variables and user-scoped directories.

## Release policy

- No distribution, GPU, driver, desktop session, or architecture is advertised as supported until installation and relevant P0 smoke workflows pass there.
- x86_64 is the first architecture target. ARM64 remains out of scope until the x86_64 product and build are stable.
- Wayland and X11 remain required desktop backends. Xwayland test results are labelled separately from native Xorg.
- Benchmark records identify distribution, kernel, CPU, RAM, GPU, userspace driver, backend, session, display size/scale, build, and fixture.
