#!/usr/bin/env python3
"""Fetch and inventory a version-pinned Windows Studio archive without running it."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath, PureWindowsPath


API_TEMPLATE = (
    "https://clientsettings.roblox.com/v2/client-version/"
    "WindowsStudio64/channel/{channel}"
)
CDN_ROOT = "https://setup.rbxcdn.com/"
SUPPORTED_CHANNEL = re.compile(r"^[A-Za-z0-9_-]{1,64}$")
UPLOAD_ID = re.compile(r"^version-[0-9a-f]{16}$")
VERSION = re.compile(r"^[0-9]+(?:\.[0-9]+){2,4}$")
MAX_DOWNLOAD_BYTES = 4 * 1024 * 1024 * 1024
MAX_EXPANDED_BYTES = 4 * 1024 * 1024 * 1024
CHUNK_SIZE = 1024 * 1024
REPO_ROOT = Path(__file__).resolve().parents[1]


class FetchError(Exception):
    pass


def require_official_host(url: str, expected_host: str) -> None:
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme != "https" or parsed.hostname != expected_host:
        raise FetchError(f"Refusing unexpected URL after redirect: {url}")


def request(url: str, *, method: str = "GET") -> urllib.request.Request:
    return urllib.request.Request(
        url,
        method=method,
        headers={"User-Agent": "roblox-studio-linux-reference-fetcher/0.1"},
    )


def get_version(channel: str) -> dict[str, object]:
    url = API_TEMPLATE.format(channel=channel)
    try:
        with urllib.request.urlopen(request(url), timeout=60) as response:
            require_official_host(response.geturl(), "clientsettings.roblox.com")
            raw = response.read(1024 * 1024 + 1)
    except (urllib.error.URLError, TimeoutError) as exc:
        raise FetchError(f"Could not read the Studio version endpoint: {exc}") from exc

    if len(raw) > 1024 * 1024:
        raise FetchError("Version endpoint response exceeded 1 MiB; refusing it.")
    try:
        data = json.loads(raw)
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise FetchError("Version endpoint did not return valid JSON.") from exc

    version = data.get("version") if isinstance(data, dict) else None
    upload_id = data.get("clientVersionUpload") if isinstance(data, dict) else None
    if not isinstance(version, str) or not VERSION.fullmatch(version):
        raise FetchError("Version endpoint schema changed: unexpected version value.")
    if not isinstance(upload_id, str) or not UPLOAD_ID.fullmatch(upload_id):
        raise FetchError(
            "Version endpoint schema changed: clientVersionUpload is not a version-<16 hex> ID."
        )

    return {
        "channel": channel,
        "version": version,
        "uploadId": upload_id,
        "apiUrl": url,
        "apiResponse": data,
    }


def cache_root(override: str | None) -> Path:
    if override:
        root = Path(override).expanduser().resolve()
    else:
        xdg_cache = os.environ.get("XDG_CACHE_HOME")
        base = Path(xdg_cache).expanduser() if xdg_cache else Path.home() / ".cache"
        root = (base / "roblox-studio-linux" / "studio-reference").resolve()

    if root == REPO_ROOT or REPO_ROOT in root.parents:
        raise FetchError("Reference binaries must be stored outside the public repository.")
    return root


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(CHUNK_SIZE):
            digest.update(chunk)
    return digest.hexdigest()


def validate_archive(archive: Path) -> tuple[int, list[str]]:
    if not zipfile.is_zipfile(archive):
        raise FetchError("Downloaded payload is not a ZIP archive.")

    with zipfile.ZipFile(archive) as bundle:
        infos = bundle.infolist()
        total_size = sum(info.file_size for info in infos)
        if total_size > MAX_EXPANDED_BYTES:
            raise FetchError(
                f"ZIP expands to {total_size} bytes, above the safety limit."
            )
        for info in infos:
            name = info.filename
            normalized = name.replace("\\", "/")
            posix_path = PurePosixPath(normalized)
            windows_path = PureWindowsPath(name)
            if (
                not name
                or "\x00" in name
                or posix_path.is_absolute()
                or windows_path.is_absolute()
                or re.match(r"^[A-Za-z]:", normalized)
                or ".." in posix_path.parts
            ):
                raise FetchError(f"Unsafe path in archive: {name!r}")
            mode = info.external_attr >> 16
            if stat.S_ISLNK(mode):
                raise FetchError(f"Refusing symbolic link in archive: {name!r}")

        bad_member = bundle.testzip()
        if bad_member is not None:
            raise FetchError(f"ZIP CRC validation failed for {bad_member!r}.")
        return len(infos), [info.filename for info in infos]


def download_archive(
    url: str, destination: Path
) -> tuple[int, str, str | None, int | None]:
    destination.parent.mkdir(parents=True, exist_ok=True)
    fd, temp_name = tempfile.mkstemp(
        prefix=".RobloxStudio.", suffix=".part", dir=destination.parent
    )
    temp_path = Path(temp_name)
    try:
        with os.fdopen(fd, "wb") as output:
            try:
                with urllib.request.urlopen(request(url), timeout=120) as response:
                    require_official_host(response.geturl(), "setup.rbxcdn.com")
                    if response.status != 200:
                        raise FetchError(f"Archive request returned HTTP {response.status}.")
                    content_type = response.headers.get_content_type()
                    if content_type != "application/zip":
                        raise FetchError(
                            f"Expected application/zip, received {content_type!r}."
                        )
                    expected_size = response.headers.get("Content-Length")
                    expected_size_int = int(expected_size) if expected_size else None
                    if expected_size_int is not None and not (
                        0 < expected_size_int <= MAX_DOWNLOAD_BYTES
                    ):
                        raise FetchError(
                            f"Archive Content-Length {expected_size_int} is outside the allowed range."
                        )
                    etag = response.headers.get("ETag")
                    digest = hashlib.sha256()
                    received = 0
                    while chunk := response.read(CHUNK_SIZE):
                        if received + len(chunk) > MAX_DOWNLOAD_BYTES:
                            raise FetchError("Archive exceeded the download size limit.")
                        output.write(chunk)
                        digest.update(chunk)
                        received += len(chunk)
                    output.flush()
                    os.fsync(output.fileno())
            except (urllib.error.URLError, TimeoutError, ValueError) as exc:
                raise FetchError(f"Archive download failed: {exc}") from exc

        if received <= 0:
            raise FetchError("The archive download was empty.")
        if expected_size_int is not None and received != expected_size_int:
            raise FetchError(
                f"Archive length mismatch: expected {expected_size_int}, got {received}."
            )
        os.replace(temp_path, destination)
        return received, digest.hexdigest(), etag, expected_size_int
    finally:
        temp_path.unlink(missing_ok=True)


def safe_extract(archive: Path, destination: Path) -> int:
    if destination.exists():
        return sum(1 for item in destination.rglob("*") if item.is_file())

    destination.parent.mkdir(parents=True, exist_ok=True)
    temp_dir = Path(tempfile.mkdtemp(prefix=".studio-extract-", dir=destination.parent))
    try:
        with zipfile.ZipFile(archive) as bundle:
            for info in bundle.infolist():
                relative = PurePosixPath(info.filename.replace("\\", "/"))
                target = (temp_dir / Path(*relative.parts)).resolve()
                if target != temp_dir and temp_dir not in target.parents:
                    raise FetchError(f"Archive member escapes extraction path: {info.filename!r}")
            bundle.extractall(temp_dir)
        os.replace(temp_dir, destination)
    except Exception:
        shutil.rmtree(temp_dir, ignore_errors=True)
        raise
    return sum(1 for item in destination.rglob("*") if item.is_file())


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Download the current Windows Studio ZIP into a private cache; never execute it."
    )
    parser.add_argument(
        "--channel",
        default="LIVE",
        help="Roblox WindowsStudio64 channel (default: LIVE).",
    )
    parser.add_argument(
        "--cache-dir",
        help="Override the XDG-based cache directory; must stay outside this repository.",
    )
    parser.add_argument(
        "--download-only",
        action="store_true",
        help="Validate and retain the ZIP without extracting it.",
    )
    args = parser.parse_args()

    if not SUPPORTED_CHANNEL.fullmatch(args.channel):
        parser.error("channel may contain only letters, digits, underscores, and hyphens")

    try:
        version_info = get_version(args.channel)
        upload_id = str(version_info["uploadId"])
        version = str(version_info["version"])
        root = cache_root(args.cache_dir)
        version_dir = root / args.channel / version / upload_id
        archive_path = version_dir / "RobloxStudio.zip"
        payload_dir = version_dir / "payload"
        manifest_path = version_dir / "manifest.json"
        archive_url = urllib.parse.urljoin(CDN_ROOT, f"{upload_id}-RobloxStudio.zip")

        version_dir.mkdir(parents=True, exist_ok=True)
        previous = None
        if archive_path.is_file() and manifest_path.is_file():
            try:
                previous = json.loads(manifest_path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                previous = None

        cache_hit = False
        if (
            isinstance(previous, dict)
            and previous.get("version") == version
            and previous.get("uploadId") == upload_id
            and previous.get("archiveUrl") == archive_url
            and previous.get("archiveSha256") == sha256_file(archive_path)
            and previous.get("zipCrcValidation") == "passed"
        ):
            downloaded_bytes = archive_path.stat().st_size
            digest = str(previous["archiveSha256"])
            etag = previous.get("archiveEtag")
            expected_content_length = previous.get("archiveContentLength", downloaded_bytes)
            member_count = int(previous.get("archiveMemberCount", 0))
            members = list(previous.get("archiveMembers", []))
            cache_hit = True
        else:
            downloaded_bytes, digest, etag, expected_content_length = download_archive(
                archive_url, archive_path
            )
            member_count, members = validate_archive(archive_path)

        extracted_file_count = None
        if not args.download_only:
            extracted_file_count = safe_extract(archive_path, payload_dir)

        binaries = []
        if payload_dir.exists():
            for binary in payload_dir.rglob("RobloxStudioBeta.exe"):
                binaries.append(
                    {
                        "path": binary.relative_to(payload_dir).as_posix(),
                        "sha256": sha256_file(binary),
                    }
                )

        manifest = {
            **version_info,
            "archiveUrl": archive_url,
            "archivePath": archive_path.name,
            "downloadedAtUtc": (
                previous.get("downloadedAtUtc")
                if cache_hit and isinstance(previous, dict)
                else datetime.now(timezone.utc).isoformat()
            ),
            "lastCheckedAtUtc": datetime.now(timezone.utc).isoformat(),
            "archiveSource": "cache" if cache_hit else "download",
            "downloadedBytes": downloaded_bytes,
            "archiveContentLength": expected_content_length,
            "archiveSha256": digest,
            "archiveEtag": etag,
            "archiveEtagIsChecksum": False,
            "zipCrcValidation": "passed",
            "archiveMemberCount": member_count,
            "archiveMembers": members,
            "payloadDirectory": payload_dir.name if payload_dir.exists() else None,
            "extractedFileCount": extracted_file_count,
            "studioBinaries": binaries,
            "executed": False,
        }
        temp_manifest = manifest_path.with_suffix(".json.tmp")
        temp_manifest.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        os.replace(temp_manifest, manifest_path)
        summary = {
            "status": "ok",
            "channel": args.channel,
            "version": version,
            "uploadId": upload_id,
            "archiveUrl": archive_url,
            "archiveSource": manifest["archiveSource"],
            "archiveBytes": downloaded_bytes,
            "archiveSha256": digest,
            "archiveEtag": etag,
            "archiveMemberCount": member_count,
            "extractedFileCount": extracted_file_count,
            "studioBinaries": binaries,
            "cacheDirectory": str(version_dir),
            "manifestPath": str(manifest_path),
            "executed": False,
        }
        print(json.dumps(summary, indent=2))
        return 0
    except (FetchError, OSError, zipfile.BadZipFile) as exc:
        print(f"fetch_studio_reference: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
