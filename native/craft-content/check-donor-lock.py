"""Check that adding the finite worker preserved every pinned donor package.

Run only at the complete index-stage validation boundary, after its build.
"""
import argparse
import hashlib
import json
from pathlib import Path
import tomllib
import zipfile


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine",required=True)
    parser.add_argument("--scratch-root",type=Path,required=True)
    parser.add_argument("--archives-root",type=Path,required=True)
    args=parser.parse_args()
    manifest=json.loads((Path(__file__).parent/"sources.json").read_text())
    app=next(app for app in manifest["apps"] if app["engine"]==args.engine)
    archive=args.archives_root/app["donor"]["sourceArchive"]
    with archive.open("rb") as source:
        if hashlib.file_digest(source,"sha256").hexdigest()!=app["donor"]["sourceSha256"]:
            raise ValueError("The pinned donor archive changed")
    with zipfile.ZipFile(archive) as source:
        locks=[name for name in source.namelist() if name.count("/")==1 and name.endswith("/Cargo.lock")]
        if len(locks)!=1:raise ValueError("The pinned source has no single root Cargo.lock")
        original=tomllib.loads(source.read(locks[0]).decode())
    current=tomllib.loads((args.scratch_root/"sources"/args.engine/"Cargo.lock").read_text())
    identity=lambda item:(item["name"],item["version"],item.get("source",""))
    packages={identity(item):item for item in current["package"]}
    def dependencies(item,lock):
        targets=[]
        for dependency in item.get("dependencies",[]):
            parts=dependency.split(" ",2)
            candidates=[package for package in lock["package"] if package["name"]==parts[0]]
            if len(parts)>1:candidates=[package for package in candidates if package["version"]==parts[1]]
            if len(parts)>2:candidates=[package for package in candidates if package.get("source","")==parts[2].removeprefix("(").removesuffix(")")]
            if len(candidates)!=1:raise ValueError(f"A pinned dependency identity is ambiguous: {dependency}")
            targets.append(identity(candidates[0]))
        return sorted(targets)
    for item in original["package"]:
        other=packages.get(identity(item))
        # Cargo may qualify an old name after adding another version. Compare
        # its resolved identity, rather than mistaking formatting for an upgrade.
        if other is None or {key:value for key,value in other.items() if key!="dependencies"}!={key:value for key,value in item.items() if key!="dependencies"} or dependencies(item,original)!=dependencies(other,current):
            raise ValueError(json.dumps(dict(pinnedDonorEntryChanged=identity(item),expected=item,current=other,
                expectedDependencies=dependencies(item,original),currentDependencies=None if other is None else dependencies(other,current))))
    print(json.dumps(dict(engine=args.engine,pinnedDonorEntriesPreserved=True,
                          addedPackages=len(current["package"])-len(original["package"]))))


if __name__=="__main__":main()
