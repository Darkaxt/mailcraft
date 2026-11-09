"""Vendor the permissive safe Rust adapters, without changing Pathweft.

This is source preparation, not a build or a qualification run. Donor source
archives stay separately pinned; no deployed Pathweft library or GPL viewer is
copied. Existing destination files must match exactly, never overwritten.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tomllib


ENGINES = ("wordcraft", "gridcraft", "deckcraft", "pdfcraft", "photocraft", "vectorcraft", "figcraft")
ROOT = Path(__file__).resolve().parent


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def retain(path: Path, data: bytes) -> None:
    if path.exists():
        if path.read_bytes() != data:
            raise ValueError(f"Preserve the differing existing source: {path}")
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("xb") as output:
        output.write(data)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pathweft-source", type=Path, required=True)
    args = parser.parse_args()
    source = args.pathweft_source.resolve(strict=True)
    manifest = json.loads((source / "vendor/Office/runtime-manifest.json").read_text(encoding="utf-8"))
    apps = {app["id"]: app for app in manifest["apps"]}
    revision = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    origin = subprocess.check_output(["git", "-C", str(source), "remote", "get-url", "origin"], text=True).strip()
    if not origin.startswith(("https://github.com/", "git@github.com:")):
        raise ValueError("The source checkout has no recognized GitHub provenance")
    licence = (source / "LICENSE").read_bytes()
    if not licence.startswith(b"MIT License"):
        raise ValueError("The adapter source licence requires a separate review")
    retain(ROOT / "LICENSE-Pathweft", licence)
    records = []
    for engine in ENGINES:
        bridge = source / "native/office" / engine
        package = tomllib.loads((bridge / "Cargo.toml").read_text(encoding="utf-8"))
        if package["package"]["license"] != "MIT":
            raise ValueError(f"The selected adapter is not declared MIT: {engine}")
        original = (bridge / "src/engine.rs").read_bytes()
        if b"unsafe " in original:
            raise ValueError(f"Do not import an unsafe ABI into the safe adapter: {engine}")
        header = b"// SPDX-FileCopyrightText: 2026 Protagonist Labs and Pathweft contributors\n// SPDX-License-Identifier: MIT\n"
        data = header + original
        retain(ROOT / "adapters" / engine / "engine.rs", data)
        app = apps[engine]
        records.append(dict(engine=engine,
            donor={key: app[key] for key in ("version", "sourceCommit", "sourceArchive", "sourceSha256", "sourceUrl") if key in app},
            fonts=app.get("fonts"), dependencies=package["dependencies"],
            adapter=dict(path=f"adapters/{engine}/engine.rs", sha256=digest(data), originalSha256=digest(original))))
    record = dict(version=1, source=dict(repository=origin, revision=revision,
        scope="Safe MIT Rust engine adapters only; exact file hashes identify any source-checkout changes"),
        licence=dict(path="LICENSE-Pathweft", sha256=digest(licence)), apps=records)
    retain(ROOT / "sources.json", (json.dumps(record, indent=2) + "\n").encode("utf-8"))
    print(json.dumps(dict(sourcePreparation=True, adapters=len(records), buildOrValidation=False)))


if __name__ == "__main__":
    main()
