"""Prepare the selected native runtime ZIP for the existing release handoff.

Attach the qualified ZIP to the matching draft before tagging the final release.
The ZIP is retained in the final checksum-bound asset set. No native code runs.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[2]
MAX_BYTES = 2 * 1024 * 1024 * 1024
MAX_MEMBERS = 1000


def download_runtime(selection, version, download_root):
    """Read one immutable, checksum-pinned asset without draft-by-tag discovery."""
    binding = selection.get("releaseInput", {})
    name = f"mailcraft-{version}-windows-x64-content-runtime.zip"
    if (binding.get("repository") != "Darkaxt/mailcraft" or binding.get("tag") != "v" + version
            or binding.get("name") != name or type(binding.get("assetId")) is not int or binding["assetId"] <= 0
            or type(binding.get("bytes")) is not int or not 0 < binding["bytes"] <= MAX_BYTES
            or not isinstance(binding.get("sha256"), str) or not re.fullmatch(r"[0-9a-f]{64}", binding["sha256"])):
        raise ValueError("Pin the exact matching release input asset ID/name/size/SHA256 before packaging")
    endpoint = f"repos/Darkaxt/mailcraft/releases/assets/{binding['assetId']}"
    api = ["gh", "api", "--hostname", "github.com", endpoint]
    metadata = json.loads(subprocess.check_output(api, text=True, encoding="utf-8"))
    if (metadata.get("id") != binding["assetId"] or metadata.get("name") != name
            or metadata.get("state") != "uploaded" or metadata.get("size") != binding["bytes"]
            or metadata.get("digest") != "sha256:" + binding["sha256"]):
        raise ValueError("GitHub runtime asset metadata differs from the committed identity")
    if download_root.exists():
        raise FileExistsError("Preserve the original download and uncertain result")
    download_root.mkdir(parents=True)
    archive = download_root / name
    with archive.open("xb") as output:
        # This read runs only in the separate protected input job: GitHub denies
        # draft assets to the compiler's read-only token. No compiler receives
        # its repository-write capability, a PAT, signing key or expiring URL.
        # Binary response bytes go only to this exclusive file, never a terminal.
        subprocess.run([*api, "--allow-escape-sequences", "--header", "Accept: application/octet-stream"],
                       stdout=output, check=True)
    with archive.open("rb") as stream:
        actual = hashlib.file_digest(stream, "sha256").hexdigest()
    if archive.stat().st_size != binding["bytes"] or actual != binding["sha256"]:
        raise ValueError("Downloaded runtime differs from its committed asset checksum; preserve original failure")
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--handoff", type=Path, required=True)
    parser.add_argument("--download-root", type=Path)
    parser.add_argument("--archive", type=Path)
    args = parser.parse_args()
    package = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]
    if package["repository"] != "https://github.com/Darkaxt/mailcraft":
        raise ValueError("Use the fork's explicitly configured release source")
    name = f"mailcraft-{package['version']}-windows-x64-content-runtime.zip"
    selection = json.loads((ROOT / "native/craft-content/release-runtime.json").read_text(encoding="utf-8"))
    archive = args.archive
    if archive is None:
        if args.download_root is None or args.download_root.exists():
            raise ValueError("Provide a new exact download scratch directory")
        archive = download_runtime(selection, package["version"], args.download_root)
    spec = importlib.util.spec_from_file_location("mailcraft_package_runtime", ROOT / "packaging/windows/content-runtime.py")
    support = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(support)
    output = args.handoff / "content-runtime"
    delivered = args.handoff / name
    if output.exists() or delivered.exists():
        raise FileExistsError("Preserve the original runtime handoff and uncertain preparation outcome")
    if archive.stat().st_size > MAX_BYTES:
        raise ValueError("Oversized runtime ZIP")
    with zipfile.ZipFile(archive) as zipped:
        infos = zipped.infolist()
        if len(infos) > MAX_MEMBERS:
            raise ValueError("Too many runtime archive members")
        manifest_info = zipped.getinfo("runtime.json")
        if manifest_info.file_size > 1024 * 1024:
            raise ValueError("Oversized runtime manifest")
        with zipped.open(manifest_info) as stream:
            manifest_bytes = stream.read(1024 * 1024 + 1)
        if hashlib.sha256(manifest_bytes).hexdigest() != selection["manifestSha256"]:
            raise ValueError("The archive does not carry the selected qualified manifest")
        manifest = json.loads(manifest_bytes)
        expected = {row["path"]: row for row in manifest["payloads"] + manifest["ocr"]["models"]}
        expected["runtime.json"] = dict(bytes=len(manifest_bytes), sha256=selection["manifestSha256"])
        if len(expected) > MAX_MEMBERS or sum(row["bytes"] for row in expected.values()) > MAX_BYTES:
            raise ValueError("Oversized qualified runtime")
        seen = set()
        files = []
        for info in infos:
            path = PurePosixPath(info.filename)
            if path.is_absolute() or not path.parts or any(part in ("..", ".") or ":" in part or "\\" in part for part in path.parts):
                raise ValueError("Unsafe runtime archive path")
            if stat.S_ISLNK(info.external_attr >> 16):
                raise ValueError("Runtime archive links are not supported")
            if info.is_dir():
                continue
            if info.filename.casefold() in seen or info.filename not in expected:
                raise ValueError("Duplicate or unlisted runtime archive member")
            seen.add(info.filename.casefold())
            if info.file_size != expected[info.filename]["bytes"]:
                raise ValueError("Runtime member size differs from the selected manifest")
            files.append(info)
        if len(seen) != len(expected):
            raise ValueError("Runtime archive is incomplete")
        output.mkdir(parents=True)
        for info in files:
            target = output.joinpath(*PurePosixPath(info.filename).parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            digest = hashlib.sha256()
            length = 0
            with zipped.open(info) as source, target.open("xb") as dest:
                while chunk := source.read(64 * 1024):
                    length += len(chunk)
                    if length > expected[info.filename]["bytes"]:
                        raise ValueError("Runtime member grew while extracting")
                    digest.update(chunk)
                    dest.write(chunk)
            if length != expected[info.filename]["bytes"] or digest.hexdigest() != expected[info.filename]["sha256"]:
                raise ValueError("Runtime member differs from its selected digest")
    support.selected(output)
    with archive.open("rb") as source, delivered.open("xb") as destination:
        shutil.copyfileobj(source, destination)
    if support.digest(delivered) != support.digest(archive):
        raise ValueError("The retained runtime archive changed during handoff")
    print(json.dumps({"runtime": str(output), "archive": str(delivered), "nativeLaunched": False}))


if __name__ == "__main__":
    main()
