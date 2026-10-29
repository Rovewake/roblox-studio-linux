#!/usr/bin/env python3
"""Fetch the exact version-pinned public Roblox API dump into the XDG cache."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "docs/research/engine-api-surface.json"
MAX_DOWNLOAD_BYTES = 64 * 1024 * 1024
CHUNK_SIZE = 1024 * 1024


class FetchError(Exception):
    pass


def cache_root(override: str | None) -> Path:
    if override:
        root = Path(override).expanduser().resolve()
    else:
        xdg_cache = os.environ.get("XDG_CACHE_HOME")
        base = Path(xdg_cache).expanduser() if xdg_cache else Path.home() / ".cache"
        root = (base / "roblox-studio-linux" / "studio-reference" / "api-dumps").resolve()

    if root == ROOT or ROOT in root.parents:
        raise FetchError("API dumps must stay outside the repository")
    return root


def require_official_host(url: str) -> None:
    try:
        parsed = urllib.parse.urlparse(url)
        trusted = (
            parsed.scheme == "https"
            and parsed.hostname == "setup.rbxcdn.com"
            and parsed.username is None
            and parsed.password is None
            and parsed.port in (None, 443)
        )
    except ValueError:
        trusted = False
    if not trusted:
        raise FetchError(f"Refusing URL outside the pinned CDN origin: {url}")


class OfficialRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(
        self,
        request: urllib.request.Request,
        file_pointer: Any,
        code: int,
        message: str,
        headers: Any,
        new_url: str,
    ) -> urllib.request.Request | None:
        require_official_host(new_url)
        return super().redirect_request(
            request, file_pointer, code, message, headers, new_url
        )


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(CHUNK_SIZE):
            digest.update(chunk)
    return digest.hexdigest()


def validate_dump(path: Path, reference: dict[str, Any]) -> None:
    try:
        with path.open("r", encoding="utf-8") as source:
            value = json.load(source)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise FetchError(f"Downloaded API dump is not valid UTF-8 JSON: {exc}") from exc
    if not isinstance(value, dict):
        raise FetchError("Downloaded API dump has an unexpected JSON root")
    if value.get("Version") != reference.get("schemaVersion"):
        raise FetchError("Downloaded API dump schema does not match the selected version")
    if not isinstance(value.get("Classes"), list) or len(value["Classes"]) != reference.get("classCount"):
        raise FetchError("Downloaded API dump class count differs from the selected manifest")
    if not isinstance(value.get("Enums"), list) or len(value["Enums"]) != reference.get("enumCount"):
        raise FetchError("Downloaded API dump enum count differs from the selected manifest")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--cache-dir",
        help="Override the XDG cache directory; it must remain outside this repository.",
    )
    args = parser.parse_args()

    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
        reference = manifest.get("reference") if isinstance(manifest, dict) else None
        if not isinstance(reference, dict):
            raise FetchError("Engine API manifest has no reference object")
        url = reference.get("apiDumpUrl")
        upload_id = reference.get("uploadId")
        expected_sha256 = reference.get("apiDumpSha256")
        expected_size = reference.get("apiDumpBytes")
        if not all(isinstance(value, str) for value in (url, upload_id, expected_sha256)):
            raise FetchError("Engine API manifest has malformed URL, upload ID, or SHA-256")
        if not re.fullmatch(r"version-[0-9a-f]{16}", upload_id):
            raise FetchError("Engine API manifest upload ID has an unexpected format")
        expected_url = f"https://setup.rbxcdn.com/channel/common/{upload_id}-API-Dump.json"
        if url != expected_url:
            raise FetchError("Engine API manifest URL does not match the pinned upload ID")
        if not isinstance(expected_size, int) or not (0 < expected_size <= MAX_DOWNLOAD_BYTES):
            raise FetchError("Engine API manifest has an invalid expected dump size")

        destination_root = cache_root(args.cache_dir)
        destination = destination_root / f"{upload_id}-API-Dump.json"
        if destination.is_symlink():
            raise FetchError("cached API dump path must not be a symlink")
        if destination.exists() and not destination.is_file():
            raise FetchError("cached API dump path must be a regular file")
        if destination.is_file() and destination.stat().st_size == expected_size:
            if sha256_file(destination) == expected_sha256:
                validate_dump(destination, reference)
                print(f"verified cached API dump: {destination}")
                return 0

        destination_root.mkdir(parents=True, exist_ok=True)
        fd, temporary_name = tempfile.mkstemp(
            prefix=f".{upload_id}-API-Dump.", suffix=".part", dir=destination_root
        )
        temporary = Path(temporary_name)
        try:
            digest = hashlib.sha256()
            received = 0
            with os.fdopen(fd, "wb") as output:
                try:
                    request = urllib.request.Request(
                        url,
                        headers={"User-Agent": "roblox-studio-linux-api-dump-fetcher/0.1"},
                    )
                    opener = urllib.request.build_opener(OfficialRedirectHandler())
                    with opener.open(request, timeout=120) as response:
                        require_official_host(response.geturl())
                        if response.status != 200:
                            raise FetchError(f"API dump request returned HTTP {response.status}")
                        while chunk := response.read(CHUNK_SIZE):
                            if received + len(chunk) > MAX_DOWNLOAD_BYTES:
                                raise FetchError("API dump exceeded the 64 MiB download limit")
                            output.write(chunk)
                            digest.update(chunk)
                            received += len(chunk)
                        output.flush()
                        os.fsync(output.fileno())
                except (urllib.error.URLError, TimeoutError) as exc:
                    raise FetchError(f"API dump download failed: {exc}") from exc

            if received != expected_size:
                raise FetchError(f"API dump length mismatch: expected {expected_size}, got {received}")
            if digest.hexdigest() != expected_sha256:
                raise FetchError(
                    f"API dump SHA-256 mismatch: expected {expected_sha256}, got {digest.hexdigest()}"
                )
            validate_dump(temporary, reference)
            os.replace(temporary, destination)
        finally:
            temporary.unlink(missing_ok=True)

        print(f"downloaded and verified API dump: {destination}")
        return 0
    except (OSError, json.JSONDecodeError, FetchError) as exc:
        print(f"Studio API dump fetch failed: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
