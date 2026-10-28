# Phase 1 REA lab and environment audit

**Date:** 2026-10-28
**Status:** version resolution, acquisition, and static package inventory complete; behavioral specifications remain in progress.
**Scope:** record the REA setup, host/provider readiness, and the acquired Windows Studio reference package. The project has no Windows test environment, so this note contains no live Studio observations. The downloaded binary was not executed, and Windows execution is not a pending project setup task.

## REA source and version

- The latest published REA release checked for this audit is `rea-agents-6.8.0`, published on 2026-10-25. Recheck the release before later setup because REA changes frequently. [REA releases](https://github.com/morluto/rea/releases/tag/rea-agents-6.8.0)
- The official package requires Node.js 22.23.4+, 24.22.1+, or 26.12.1+, plus npm. The host Node.js is 24.21.0, below the supported 24.x minimum. Read-only REA CLI operations were run using Node.js 26.12.1 and npm 11.20.0 from temporary Nix shells; no system or user profile was changed for this runtime. [REA README](https://github.com/morluto/rea/blob/main/README.md)
- REA's Linux process supervisor needs a procps-compatible `ps`. This host has procps-ng 4.0.6.
- The host is NixOS 26.05, x86_64. REA 6.8.0's general `doctor --json` check reports `unsupported_host`: its documented Linux host support targets distributions with a recognized apt, dnf, or pacman package family. This is a host-support limitation; it did not prevent the read-only package inventory or scoped Ghidra provider check below. [REA installation and setup](https://github.com/morluto/rea/blob/main/docs/installation.md)

## Provider readiness

- REA's native guide lists Ghidra 12.1.x and a 64-bit JDK 21 or newer on Linux. A temporary Nix shell provided Ghidra 12.1.4 and JDK 21.0.12.1+1. REA's provider-scoped doctor checks passed for Ghidra, Java, Linux/x64, and the native decompiler. The tools were not installed to the user profile. [REA native analysis guide](https://rea.tools/guides/native/)
- The general REA doctor still reports NixOS as outside its supported host boundary. Treat provider availability on this host as an exploratory result, not a claim that REA supports NixOS.
- Hopper was not selected: its automated Linux setup is documented for a narrower set of x86_64 distributions, and it is unnecessary for this initial investigation. The host has no IDA registration.

## REA setup review

- Ran the official `rea-agents@6.8.0` Codex setup in dry-run mode with `--client codex --dry-run --json`.
- The reviewed plan would add an REA MCP registration to the user's Codex configuration and install REA's shared workflow instructions. The plan was not applied; the direct CLI was sufficient for this read-only investigation, and setup would modify user-level agent configuration.
- REA's `inspect_desktop_application` inventory was run against the extracted package directory. It read 14 files, probed 9 files, inspected 3,060 bytes, found zero symlinks, and reported zero components, candidates, or next probes. This is a limited result for the downloaded ZIP payload, not a full Windows installation inventory or evidence that the application has no further components. The evidence bundle is kept in the private reference cache.

## Windows Studio reference acquisition

- **Observation:** the official client-version endpoint `https://clientsettings.roblox.com/v2/client-version/WindowsStudio64/channel/LIVE` returned version `0.745.0.7451137` and upload ID `version-7062aec9dbe74b14` on 2026-10-28. The endpoint is empirically observed and is not documented here as a stable public API.
- **Observation:** the version-pinned archive URL was `https://setup.rbxcdn.com/version-7062aec9dbe74b14-RobloxStudio.zip`. The download completed over HTTPS with 120,313,638 bytes and passed ZIP CRC validation.
- **Observation:** locally calculated archive SHA-256: `804414bb9b99a7149bd914a4226da043c6eba7faa57bbf01a52bc68ed729280a`.
- **Observation:** archive ETag was `"fc8b526fe17bf6e88e6394cb48cc1bfa"`. This is recorded as an HTTP validator, not as a cryptographic checksum.
- **Observation:** extracted `RobloxStudioBeta.exe` SHA-256: `781b36ed6fb62e61292b9e6a64327280c7da12b00f3ea8b2c91a3f4834f860ec`.
- Acquisition and extraction were performed by `scripts/fetch_studio_reference.py` into the user's XDG cache under `roblox-studio-linux/studio-reference/`. No archive or extracted proprietary file is tracked by Git.
- Re-run from the repository root with `python3 scripts/fetch_studio_reference.py --channel LIVE`; use `--download-only` to retain the archive without extraction. The script re-resolves the selected channel each time and creates a version-specific cache directory.
- The metadata-only version baseline is recorded in `studio-reference-baseline.json`; scheduled update checks should compare upload IDs against this file and leave all downloaded content in the private cache.
- The script was run a second time and correctly reused the verified local cache. The manifest records the endpoint response, channel, version/upload ID, archive URL, ETag, sizes, hashes, ZIP CRC result, and executable hash.
- The download was not executed. No Windows compatibility runtime was used. Do not infer runtime dependencies or interactive behavior from the archive alone.

## Static investigation

- **Observation:** REA's Ghidra provider was configured ephemerally and its provider-specific checks passed. A read-only string search for `.rbxl` in `RobloxStudioBeta.exe` was attempted with `rea search ... --kind strings --provider ghidra`. The first run exceeded REA's 330-second import/analyze deadline; a retry with a 900-second startup timeout also ended with `provider_timeout`. Both results were explicitly incomplete and contain no usable search finding. No `OutOfMemoryError` was confirmed; a memory cause cannot be ruled out.
- **Limitation:** the downloaded ZIP is a versioned package, not a live native Windows Studio session. No startup, open/save, hierarchy, property editor, Luau editor, viewport, undo/redo, or playtest behavior has been observed. Direct Windows tests are unavailable to this project and must not be treated as a future gate.
- **Unknown:** GPU/driver behavior, runtime-loaded components, install-time state, user configuration, and behavior across Studio versions.

## Phase 1 gate assessment

**Not passed.** Phase 1 has a repeatable, version-pinned download path, recorded package provenance, and an initial static inventory. Its P0 behavior contracts and evidence notes are still incomplete. Build them from technical documentation, successful static evidence, public format specifications, fixtures, and creator feedback. Mark gaps as unknown; do not require or claim Windows tests. The P0 workflows in `phase-1-p0-workflow-plan.md` remain investigation questions, not findings.

## Next steps

1. Work through P0-01 to P0-08 using technical documents, static package evidence, public format specifications, and fixtures; separate observations from interpretation and hypotheses.
2. If a specific static question remains useful, retry with a narrower analyzer profile or a smaller binary; do not treat either timed-out result as evidence.
3. Build independent compatibility checks from documented contracts, public fixtures, and open-source implementations; state the coverage and limits of each.
4. Before treating the acquisition script as update automation, revalidate the current client-version response schema and matching versioned archive behavior. Later add the scheduled GitHub Actions check described in `ROADMAP.md`; it must publish metadata only and must not retain Roblox binaries in GitHub storage.

## Sources

- [REA 6.8.0 release](https://github.com/morluto/rea/releases/tag/rea-agents-6.8.0)
- [REA official README](https://github.com/morluto/rea/blob/main/README.md)
- [REA installation and setup](https://github.com/morluto/rea/blob/main/docs/installation.md)
- [REA native analysis guide](https://rea.tools/guides/native/)
- [REA desktop application inspection](https://github.com/morluto/rea/blob/main/docs/desktop-applications.md)
- [Roblox Studio setup documentation](https://create.roblox.com/docs/studio/setup) — technical installation documentation.
- [Roblox Studio CLI documentation](https://create.roblox.com/docs/studio/cli) — technical reference for Studio's command-line interface and installation paths.
- [Observed WindowsStudio64 LIVE version endpoint](https://clientsettings.roblox.com/v2/client-version/WindowsStudio64/channel/LIVE) — direct technical endpoint queried for this reference; not assumed to be a stable public API.
