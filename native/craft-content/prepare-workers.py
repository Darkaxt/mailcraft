"""Prepare exact pinned Craft source/contract inputs, without building or testing.

Only a registered, explicit D:/Temp compiler root is accepted. Existing sources
are never swept or replaced. Shared Pathweft archives are read-only inputs.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import stat
import tomllib
import zipfile


ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[1]


def sha(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write(path: Path, data: bytes) -> None:
    if path.exists():
        if path.read_bytes() != data:
            raise ValueError(f"Preserve the differing prepared input: {path}")
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("xb") as output:
        output.write(data)


def extract(archive_path: Path, expected: str, destination: Path) -> None:
    if sha(archive_path) != expected:
        raise ValueError(f"Pinned source archive changed: {archive_path}")
    if destination.exists():
        raise FileExistsError(f"Preserve the existing compiler input tree: {destination}")
    with zipfile.ZipFile(archive_path) as archive:
        roots = {PurePosixPath(member.filename).parts[0] for member in archive.infolist()}
        if len(roots) != 1:
            raise ValueError("The source archive has no single upstream root")
        for member in archive.infolist():
            parts = PurePosixPath(member.filename).parts[1:]
            if not parts:
                continue
            if any(part in ("..", ".") or ":" in part or "\\" in part for part in parts):
                raise ValueError("Unsafe source archive member")
            if stat.S_ISLNK(member.external_attr >> 16):
                raise ValueError("Source archive links require explicit handling")
            target = destination.joinpath(*parts)
            if not target.resolve().is_relative_to(destination.resolve()):
                raise ValueError("Source archive escaped its exact compiler root")
            if member.is_dir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with archive.open(member) as source, target.open("xb") as output:
                    shutil.copyfileobj(source, output)


def toml(value) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, str):
        return json.dumps(value)
    if isinstance(value, list):
        return "[" + ",".join(toml(item) for item in value) + "]"
    if isinstance(value, dict):
        return "{" + ",".join(f"{json.dumps(key)}={toml(item)}" for key, item in value.items()) + "}"
    raise ValueError(f"Unsupported Cargo value: {value!r}")


def contract(destination: Path) -> list[dict]:
    source = REPO / "crates/craft-content"
    # Workers consume only the location, paging and protocol modules. Runtime
    # supervision/OCR/HTML remain optional modules for actual consumers.
    package = '''[package]
name = "craft-content"
version = "0.1.0"
edition = "2024"
license = "MIT OR Apache-2.0"
[features]
default = []
html = []
runtime = []
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
'''
    write(destination / "Cargo.toml", package.encode())
    records = []
    for name in ("lib.rs", "paging.rs", "protocol.rs"):
        original = source / "src" / name
        target = destination / "src" / name
        write(target, original.read_bytes())
        records.append(dict(path=f"crates/craft-content/src/{name}", sha256=sha(original)))
    for name in ("LICENSE-MIT", "LICENSE-APACHE"):
        original = REPO / name
        if original.exists():
            write(destination / name, original.read_bytes())
    return records


def bridge_manifest(app, workspace, shared):
    engine = app["engine"]
    dependencies = dict(app["dependencies"])
    for name, fallback in (("serde_json", "1"), ("image", {"version":"0.25", "default-features":False,"features":["png"]}),
                           ("ring", "0.17"), ("tempfile", "3"), ("zip", {"version":"6", "default-features":False})):
        if name not in dependencies:
            dependencies[name] = {"workspace":True} if name in workspace.get("workspace", {}).get("dependencies", {}) else fallback
    dependencies["craft-content"] = {"path":shared.as_posix(),"default-features":False}
    package = f'''[package]
name = "mailcraft-content-{engine}"
version = "0.1.0"
edition = "2024"
license = "MIT OR Apache-2.0"
[[bin]]
name = "mailcraft-content-{engine}"
path = "src/main.rs"
[dependencies]
'''
    return (package + "".join(f"{json.dumps(name)}={toml(value)}\n" for name,value in dependencies.items())).encode()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scratch-root", type=Path, required=True)
    parser.add_argument("--archives-root", type=Path, required=True)
    parser.add_argument("--registration", type=Path, required=True,
                        help="Task-owned record naming this exact registered compiler scratch root")
    parser.add_argument("--refresh-bridge", help="Inspected compiler failure/correction; refresh only recorded, unchanged bridge preimages")
    parser.add_argument("--restore-donor-lock", help="Restore one inspected generated lock to its exact verified donor archive baseline")
    parser.add_argument("--failure-evidence", type=Path, help="New retained directory for the exact failed lock/bridge inputs before restoration")
    args = parser.parse_args()
    scratch = args.scratch_root.resolve()
    registration = json.loads(args.registration.read_text(encoding="utf-8"))
    if scratch.parent != Path("D:/Temp").resolve() or not scratch.name.startswith("mailcraft-index-"):
        raise ValueError("Use the exact task-owned D: native compiler scratch root")
    if registration.get("scratchRoot") != str(scratch) or not registration.get("cleanupTransaction"):
        raise ValueError("Register and record the exact compiler root before preparation")
    manifest = json.loads((ROOT / "sources.json").read_text(encoding="utf-8"))
    if args.refresh_bridge:
        inputs_path = scratch / "source-inputs.json"
        inputs = json.loads(inputs_path.read_text(encoding="utf-8"))
        if args.restore_donor_lock:
            if args.failure_evidence is None or not args.failure_evidence.is_absolute():
                raise ValueError("Provide an explicit retained failure-input evidence directory")
            app = next(item for item in inputs["apps"] if item["engine"] == args.restore_donor_lock)
            definition = next(item for item in manifest["apps"] if item["engine"] == app["engine"])
            archive = args.archives_root / definition["donor"]["sourceArchive"]
            if sha(archive) != definition["donor"]["sourceSha256"]:
                raise ValueError("The exact pinned donor archive changed")
            with zipfile.ZipFile(archive) as source:
                locks = [name for name in source.namelist() if name.count("/") == 1 and name.endswith("/Cargo.lock")]
                if len(locks) != 1: raise ValueError("No single original root Cargo.lock")
                baseline = source.read(locks[0])
            if hashlib.sha256(baseline).hexdigest() != app["originalLockSha256"]:
                raise ValueError("The original donor lock identity differs from preparation")
            lock = scratch / "sources" / app["engine"] / "Cargo.lock"
            before = sha(lock)
            args.failure_evidence.mkdir(parents=True, exist_ok=False)
            shutil.copyfile(lock, args.failure_evidence / "failed.Cargo.lock")
            shutil.copyfile(lock.parent / "apps/mailcraft-content/Cargo.toml", args.failure_evidence / "failed-worker.Cargo.toml")
            shutil.copyfile(lock.parent / "apps/mailcraft-content/src/main.rs", args.failure_evidence / "failed-worker-main.rs")
            shutil.copyfile(inputs_path, args.failure_evidence / "failed-source-inputs.json")
            lock.write_bytes(baseline)
            inputs.setdefault("lockRestorations", []).append(dict(engine=app["engine"],before=before,after=sha(lock),
                archiveSha256=definition["donor"]["sourceSha256"],reason=args.refresh_bridge,evidence=str(args.failure_evidence)))
        changes = []
        retired = []
        for app in inputs["apps"]:
            old_app = json.loads(json.dumps(app))
            first_change = len(changes)
            definition = next(item for item in manifest["apps"] if item["engine"] == app["engine"])
            bridge = scratch / "sources" / app["engine"] / "apps/mailcraft-content"
            for record in app["workerInputs"]:
                target = bridge / record["path"]
                if sha(target) != record["sha256"]:
                    raise ValueError(f"Preserve the independently changed prepared input: {target}")
            package = bridge_manifest(definition, tomllib.loads((bridge.parents[1] / "Cargo.toml").read_text(encoding="utf-8")), scratch / "contract")
            record = next(item for item in app["workerInputs"] if item["path"] == "Cargo.toml")
            current = hashlib.sha256(package).hexdigest()
            if current != record["sha256"]:
                changes.append(dict(engine=app["engine"],path="Cargo.toml",before=record["sha256"],after=current))
                (bridge / "Cargo.toml").write_bytes(package)
                record["sha256"] = current
            for original, relative in ((ROOT / "worker.rs", "src/main.rs"),
                                       (ROOT / "extraction.rs", "src/extraction.rs"),
                                       (ROOT / definition["adapter"]["path"], "src/engine.rs")):
                record = next(item for item in app["workerInputs"] if item["path"] == relative)
                current = sha(original)
                if current != record["sha256"]:
                    changes.append(dict(engine=app["engine"], path=relative, before=record["sha256"], after=current))
                    (bridge / relative).write_bytes(original.read_bytes())
                    record["sha256"] = current
            app["adapterCurrentSha256"] = sha(ROOT / definition["adapter"]["path"])
            receipt = scratch / "receipts" / f'{app["engine"]}-build.json'
            if len(changes) != first_change and receipt.exists():
                prior = json.loads(receipt.read_text(encoding="utf-8-sig"))
                binary = Path(prior["executable"])
                if (prior["input"] != old_app or prior["buildExit"] != 0 or not binary.resolve().is_relative_to(scratch / "target/native")
                    or sha(binary) != prior["sha256"] or sha(Path(prior["log"])) != prior["logSha256"]):
                    raise ValueError("Preserve a changed/unattributed prior core build before refreshing source")
                history = receipt.with_name(f'{app["engine"]}-build-source-refresh{len(inputs.get("bridgeRefreshes", []))+1}.json')
                if history.exists(): raise FileExistsError("Preserve the existing prior-build history")
                record = dict(engine=app["engine"],receipt=str(history),sha256=sha(receipt),
                              payloadSha256=prior["sha256"],reason="Expected source changed; recompile before qualification")
                receipt.rename(history)
                retired.append(record)
        inputs.setdefault("bridgeRefreshes", []).append(dict(reason=args.refresh_bridge, changes=changes, retiredBuildReceipts=retired))
        inputs["contractManifestSha256"] = sha(scratch / "contract/Cargo.toml")
        inputs_path.write_text(json.dumps(inputs, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(dict(refreshed=len(changes), nativeExecution=False, reason=args.refresh_bridge)))
        return
    shared = scratch / "contract"
    contract_inputs = contract(shared)
    records = []
    for app in manifest["apps"]:
        engine = app["engine"]
        donor = app["donor"]
        source = scratch / "sources" / engine
        extract(args.archives_root / donor["sourceArchive"], donor["sourceSha256"], source)
        original_lock = sha(source / "Cargo.lock")
        workspace = tomllib.loads((source / "Cargo.toml").read_text(encoding="utf-8"))
        bridge = source / "apps/mailcraft-content"
        write(bridge / "Cargo.toml", bridge_manifest(app, workspace, shared))
        for original, name in ((ROOT / "worker.rs", "main.rs"), (ROOT / "extraction.rs", "extraction.rs"),
                               (ROOT / app["adapter"]["path"], "engine.rs")):
            write(bridge / "src" / name, original.read_bytes())
        members = workspace.get("workspace", {}).get("members", [])
        import fnmatch
        if not any(fnmatch.fnmatch("apps/mailcraft-content", pattern) for pattern in members):
            raise ValueError(f"Donor workspace must explicitly include apps/mailcraft-content: {engine}")
        patches = []
        if engine == "figcraft":
            file = source / "crates/pathops/src/budget.rs"
            before = file.read_bytes()
            if hashlib.sha256(before).hexdigest() != "e7104635bd3c9c4cd898029973f13d58e8a862e579f8aae88d929094663efb4d" or before.count(b"i.isolate_lowest_one()") != 2:
                raise ValueError("The pinned FigCraft stable-Rust compatibility preimage changed")
            after = before.replace(b"i.isolate_lowest_one()", b"(i & i.wrapping_neg())")
            file.write_bytes(after)
            patches.append(dict(path="crates/pathops/src/budget.rs", before=hashlib.sha256(before).hexdigest(), after=sha(file)))
        fonts = app.get("fonts")
        if fonts:
            extract(args.archives_root / fonts["archive"], fonts["archiveSha256"], scratch / "fonts" / engine)
        records.append(dict(engine=engine, donor=donor, source=str(source), originalLockSha256=original_lock,
                            compatibilityPatches=patches, fonts=fonts,
                            adapterBaseSha256=app["adapter"]["sha256"], adapterCurrentSha256=sha(ROOT / app["adapter"]["path"]),
                            workerInputs=[dict(path=file.relative_to(bridge).as_posix(), sha256=sha(file)) for file in sorted(bridge.rglob("*")) if file.is_file()]))
    record = dict(version=1, sourcePreparation=True, buildOrValidation=False, scratchRoot=str(scratch),
                  storageException="D: sustained compiler/small-file IO; exact registered scratch, not a deliverable",
                  contractInputs=contract_inputs, contractManifestSha256=sha(shared / "Cargo.toml"), apps=records)
    write(scratch / "source-inputs.json", (json.dumps(record, indent=2)+"\n").encode())
    print(json.dumps(dict(sourcePreparation=True, workers=len(records), buildOrValidation=False)))


if __name__ == "__main__":
    main()
