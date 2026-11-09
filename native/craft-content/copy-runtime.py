"""Copy only a complete manifest-bound runtime into a new product package.

No native program is launched and no existing file is replaced or removed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath


def plain(path):
    info=path.lstat()
    if path.is_symlink() or getattr(info,"st_file_attributes",0)&0x400:
        raise ValueError(f"A runtime/package path is a reparse object: {path}")
    return info


def files(root):
    pending=[root]
    while pending:
        folder=pending.pop()
        plain(folder)
        with os.scandir(folder) as entries:
            for entry in entries:
                path=Path(entry.path)
                info=plain(path)
                if entry.is_dir(follow_symlinks=False):pending.append(path)
                elif entry.is_file(follow_symlinks=False):yield path
                else:raise ValueError(f"Unexpected runtime member type: {path}")


def copy_runtime(source,destination):
    plain(source)
    source=source.resolve(strict=True)
    destination=destination.absolute()
    if destination.exists():raise FileExistsError("Preserve the existing packaged runtime")
    for parent in destination.parents:
        if parent.exists():plain(parent)
    manifest_bytes=(source/"runtime.json").read_bytes()
    if len(manifest_bytes)>1024*1024:raise ValueError("Oversized native runtime manifest")
    manifest=json.loads(manifest_bytes)
    if manifest["version"]!=1 or {core["id"] for core in manifest["cores"]}!={"wordcraft","gridcraft","deckcraft","pdfcraft","photocraft","vectorcraft","figcraft"}:
        raise ValueError("The required seven native core bindings are incomplete")
    if set(manifest["ocr"]["languages"])!={"eng","spa","ell"}:
        raise ValueError("The qualified OCR language set is incomplete")
    members={}
    for member in manifest["payloads"]+manifest["ocr"]["models"]:
        raw=member["path"]
        path=PurePosixPath(raw)
        if path.is_absolute() or not path.parts or any(part in (".","..") or ":" in part or "\\" in part for part in path.parts):
            raise ValueError("Runtime manifest names an unsafe package member")
        folded=raw.casefold()
        if folded in members:raise ValueError("Duplicate runtime payload name")
        members[folded]=member
    for executable in [core["executable"] for core in manifest["cores"]]+[manifest["ocr"]["executable"]]:
        if executable.casefold() not in members:raise ValueError("A runtime executable is not integrity bound")
    expected={member["path"] for member in members.values()}|{"runtime.json"}
    actual={path.relative_to(source).as_posix() for path in files(source)}
    if actual!=expected:raise ValueError("Runtime contains missing or unlisted files; nothing extra is packaged")
    rows=[]
    for member in members.values():
        original=source.joinpath(*PurePosixPath(member["path"]).parts)
        target=destination.joinpath(*PurePosixPath(member["path"]).parts)
        plain(original)
        target.parent.mkdir(parents=True,exist_ok=True)
        digest=hashlib.sha256()
        length=0
        with original.open("rb") as input,target.open("xb") as output:
            while chunk:=input.read(64*1024):
                length+=len(chunk)
                if length>member["bytes"]:raise ValueError("Runtime payload grew while packaging")
                digest.update(chunk);output.write(chunk)
            output.flush();os.fsync(output.fileno())
        if length!=member["bytes"] or digest.hexdigest()!=member["sha256"]:
            raise ValueError(f"Runtime payload differs from its selected identity: {member['path']}")
        rows.append(dict(path=member["path"],bytes=length,sha256=digest.hexdigest()))
    target=destination/"runtime.json"
    with target.open("xb") as output:
        output.write(manifest_bytes);output.flush();os.fsync(output.fileno())
    rows.append(dict(path="runtime.json",bytes=len(manifest_bytes),sha256=hashlib.sha256(manifest_bytes).hexdigest()))
    return dict(copied=True,nativeLaunched=False,payload=rows)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source",type=Path,required=True)
    parser.add_argument("--destination",type=Path,required=True)
    args=parser.parse_args()
    print(json.dumps(copy_runtime(args.source,args.destination)))


if __name__=="__main__":main()
