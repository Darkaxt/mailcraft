"""Prepare pinned local-only OCR inputs and an isolated release-only vcpkg overlay.

This is source preparation. It does not launch a compiler, installer, GUI or OCR.
The installed vcpkg checkout is read-only; generated trees stay in exact task scratch.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
from urllib.request import Request, urlopen


ROOT=Path(__file__).resolve().parent


def sha(path:Path)->str:
    with path.open("rb") as source:return hashlib.file_digest(source,"sha256").hexdigest()


def retain(path:Path,data:bytes)->None:
    if path.exists():
        if path.read_bytes()!=data:raise ValueError(f"Preserve the different prepared input: {path}")
        return
    path.parent.mkdir(parents=True,exist_ok=True)
    with path.open("xb") as output:output.write(data)


def obtain(record:dict,target:Path)->None:
    if target.exists():
        if target.stat().st_size!=record["bytes"] or sha(target)!=record["sha256"]:
            raise ValueError(f"The prepared official OCR input changed: {target}")
        return
    target.parent.mkdir(parents=True,exist_ok=True)
    temporary=target.with_name(target.name+".partial")
    digest=hashlib.sha256()
    total=0
    with urlopen(Request(record["url"],headers={"User-Agent":"MailCraft-source-preparation"})) as response,temporary.open("xb") as output:
        while chunk:=response.read(64*1024):
            total+=len(chunk)
            if total>record["bytes"]:raise ValueError("Official OCR input grew beyond its pinned length")
            digest.update(chunk)
            output.write(chunk)
    if total!=record["bytes"] or digest.hexdigest()!=record["sha256"]:
        raise ValueError("Official OCR input does not match its exact pinned source")
    temporary.rename(target)


def main()->None:
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scratch-root",type=Path,required=True)
    parser.add_argument("--registration",type=Path,required=True)
    parser.add_argument("--vcpkg-root",type=Path,required=True)
    args=parser.parse_args()
    scratch=args.scratch_root.resolve()
    registration=json.loads(args.registration.read_text(encoding="utf-8"))
    if scratch.parent!=Path("D:/Temp").resolve() or not scratch.name.startswith("mailcraft-index-"):
        raise ValueError("Use the exact task-owned D: native compiler scratch root")
    if registration.get("scratchRoot")!=str(scratch) or not registration.get("cleanupTransaction"):
        raise ValueError("Record this exact registered scratch root first")
    pins=json.loads((ROOT/"ocr-sources.json").read_text(encoding="utf-8"))
    vcpkg=args.vcpkg_root.resolve(strict=True)
    origin=subprocess.check_output(["git","-C",str(vcpkg),"remote","get-url","origin"],text=True).strip()
    if origin.removesuffix(".git").lower() not in ("https://github.com/microsoft/vcpkg","git@github.com:microsoft/vcpkg"):
        raise ValueError("Use the existing official Microsoft vcpkg checkout")
    revision=subprocess.check_output(["git","-C",str(vcpkg),"rev-parse","HEAD"],text=True).strip()
    port=vcpkg/"ports/tesseract"
    metadata=json.loads((port/"vcpkg.json").read_text(encoding="utf-8"))
    if metadata["version"]!="5.5.3":
        raise ValueError("Reconcile the installed vcpkg port against this reviewed source preparation")
    metadata["version"]=pins["tesseract"]["version"]
    metadata.pop("port-version",None)
    metadata["default-features"]=[]
    overlay=scratch/"ocr/ports/tesseract"
    retain(overlay/"vcpkg.json",(json.dumps(metadata,indent=2)+"\n").encode())
    original=(port/"portfile.cmake").read_text(encoding="utf-8")
    prepared,count=re.subn(r'REF "\$\{VERSION\}"',f'REF "{pins["tesseract"]["commit"]}"',original,count=1)
    if count!=1:raise ValueError("The installed port no longer has its reviewed source selector")
    prepared,count=re.subn(r"SHA512 [0-9a-f]{128}",f'SHA512 {pins["tesseract"]["archive"]["sha512"]}',prepared,count=1)
    if count!=1:raise ValueError("The installed port no longer has its reviewed archive selector")
    line="        fix-msvc-training-tools.patch # tesseract-ocr/tesseract#4590\n"
    if prepared.count(line)!=1:raise ValueError("Reconcile the upstream-merged training compatibility patch")
    prepared=prepared.replace(line,"")
    options="""        -DGRAPHICS_DISABLED=ON
        -DDISABLED_LEGACY_ENGINE=ON
        -DOPENMP_BUILD=OFF
        -DBUILD_TESTS=OFF
        -DENABLE_CCACHE=OFF
"""
    needle="        -DUSE_SYSTEM_ICU=True\n"
    if prepared.count(needle)!=1:raise ValueError("The installed port's configure options changed")
    prepared=prepared.replace(needle,needle+options)
    retain(overlay/"portfile.cmake",prepared.encode())
    inputs=[]
    for file in sorted(port.iterdir()):
        if not file.is_file():continue
        inputs.append(dict(path=file.name,sha256=sha(file)))
        if file.name not in ("vcpkg.json","portfile.cmake","fix-msvc-training-tools.patch"):
            retain(overlay/file.name,file.read_bytes())
    triplet='''set(VCPKG_TARGET_ARCHITECTURE x64)
set(VCPKG_CRT_LINKAGE static)
set(VCPKG_LIBRARY_LINKAGE static)
set(VCPKG_BUILD_TYPE release)
'''
    retain(scratch/"ocr/triplets/mailcraft-x64.cmake",triplet.encode())
    manifest=dict(name="mailcraft-content-ocr",version="1.0.0",dependencies=[dict(name="tesseract", **{"default-features":False})])
    retain(scratch/"ocr/vcpkg.json",(json.dumps(manifest,indent=2)+"\n").encode())
    for model in pins["models"]["files"]:
        obtain(model,scratch/"ocr/models"/model["path"])
    for notice in pins["licences"]:
        obtain(notice,scratch/"ocr/notices"/notice["repository"]/"LICENSE")
    record=dict(version=1,sourcePreparation=True,nativeExecution=False,sourcePins=pins,
                vcpkg=dict(root=str(vcpkg),source=origin,revision=revision,portInputs=inputs),
                isolation="Exact task-owned install/build/package trees; installed ports/settings unchanged",
                buildOptions=dict(releaseOnly=True,compilerJobs=1,graphics=False,legacyEngine=False,curl=False,archive=False,trainingTools=False))
    retain(scratch/"ocr/source-inputs.json",(json.dumps(record,indent=2)+"\n").encode())
    print(json.dumps(dict(sourcePreparation=True,nativeExecution=False,version=pins["tesseract"]["version"],languages=[model["language"] for model in pins["models"]["files"]])))


if __name__=="__main__":main()
