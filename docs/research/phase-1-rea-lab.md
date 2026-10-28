# Phase 1 REA lab and environment audit

**Date:** 2026-10-28
**Status:** initial source and host audit complete; REA/provider setup and Studio behavior investigation remain blocked.
**Scope:** record verified REA requirements and the current analysis environment. No Roblox Studio binary was analyzed, and no Studio behavior is claimed as an observation in this note.

## REA source and version

- The latest published REA release checked for this audit is `rea-agents-6.8.0`, published on 2026-10-25. Confirm the release again before a later setup because REA changes frequently. [REA releases](https://github.com/morluto/rea/releases/tag/rea-agents-6.8.0)
- The official package requires Node.js 22.23.4+, 24.22.1+, or 26.12.1+, plus npm. The host Node.js is 24.21.0, below the supported 24.x minimum. Read-only REA CLI checks were run using Node.js 26.12.1 and npm 11.20.0 from a temporary Nix shell; no system or user profile was changed for this runtime. [REA README](https://github.com/morluto/rea/blob/main/README.md)
- REA's Linux process supervisor needs a procps-compatible `ps`. This host has procps-ng 4.0.6.
- The host is NixOS 26.05, x86_64. REA 6.8.0's `doctor --json` reports `unsupported_host`: its published Linux host support targets distributions with a recognized apt, dnf, or pacman package family. Therefore, this host is outside REA's documented support boundary even though Node.js and `ps` are available. [REA installation and setup](https://github.com/morluto/rea/blob/main/docs/installation.md)

## Provider readiness

- Ghidra is the preferred free native-analysis provider for this project. REA documents Ghidra 12.1.x and a 64-bit JDK 21 or newer on Linux. The queried Nixpkgs package versions are Ghidra 12.1.4 and JDK 21.0.12.1+1, but neither is installed or configured on this host. The REA doctor reports `GHIDRA_INSTALL_DIR` unset and Ghidra unavailable. [REA native analysis guide](https://rea.tools/guides/native/)
- Hopper is not selected: its automated Linux setup is documented only for a specific set of x86_64 distributions, and REA treats its installation as a separate approval. This NixOS host is not on that list.
- The host has no IDA registration. No provider has been installed, selected, or invoked.

## REA setup review

- Ran the official `rea-agents@6.8.0` Codex setup in dry-run mode with `--client codex --dry-run --json`.
- The reviewed plan would add an REA MCP registration to the user's Codex configuration and install REA's shared workflow instructions. The plan was not applied: the current host is outside REA's supported host boundary, no native provider is configured, and the target installation is unavailable.
- `rea doctor --json` confirmed Node.js 26.12.1 readiness, but reported the host as unsupported, Ghidra unavailable, and the REA Codex registration/skill missing. No Codex configuration or skill files were modified.
- REA's built-in `inspect_desktop_application` can inventory an available installation directory without a native provider and without running the application. It reads directory entries, selected headers, and bounded metadata; it does not change target files. [Desktop application inspection](https://github.com/morluto/rea/blob/main/docs/desktop-applications.md)

## Target availability

- A read-only search of the workspace and available home/removable-media locations found no Roblox Studio Windows installation or `RobloxStudioBeta.exe` target.
- No native Windows Studio session is connected to this Linux workspace. The reference Studio version, executable hashes, install inventory, GPU/driver, and observed workflows are therefore unknown.
- No Windows compatibility runtime was used. No target binary was downloaded, executed, or analyzed.

## Phase 1 gate assessment

**Not passed.** The REA sources and host requirements are documented, but the native provider is not ready under the current host's support status, and the Studio reference installation is unavailable. There are no Studio-specific observations or REA evidence to report yet. Do not promote the workflow plans in `phase-1-p0-workflow-plan.md` to findings until they have been checked against the original application.

## Requirements to resume

1. Make a native Windows Studio reference installation available for read-only inventory and live observation of local, disposable projects.
2. Provide an analysis host within REA's documented Linux support boundary, or another REA-supported analysis environment that can access the installation files. It needs Node.js at a supported version, npm, procps-compatible `ps`, Ghidra 12.1.x, and 64-bit JDK 21 or newer.
3. Re-run `rea doctor --json`, review and apply the exact Codex setup plan on that supported host, then record REA/provider/target versions and hashes before investigation.

## Sources

- [REA 6.8.0 release](https://github.com/morluto/rea/releases/tag/rea-agents-6.8.0)
- [REA official README](https://github.com/morluto/rea/blob/main/README.md)
- [REA installation and setup](https://github.com/morluto/rea/blob/main/docs/installation.md)
- [REA native analysis guide](https://rea.tools/guides/native/)
- [REA desktop application inspection](https://github.com/morluto/rea/blob/main/docs/desktop-applications.md)
