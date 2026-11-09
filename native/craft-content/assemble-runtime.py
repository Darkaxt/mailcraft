"""Assemble the private qualification payload from exact completed build receipts.

No native code is run. This is packaging, not qualification or release approval.
The requested new output directory is retained for its explicit delivery purpose.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shutil


ROOT=Path(__file__).resolve().parent
EXTENSIONS={
    "wordcraft":["docx","docm","doc","odt"],
    "gridcraft":["xlsx","xlsm","xls","xlsb","ods"],
    "deckcraft":["pptx"],"pdfcraft":["pdf"],
    "photocraft":["pcraft","psd","psb"],
    "vectorcraft":["vectorcraft","drawcraft","svg","svgz","ai"],
    "figcraft":["figcraft","fig"],
}


def identity(file:Path)->dict:
    with file.open("rb") as source:digest=hashlib.file_digest(source,"sha256").hexdigest()
    return dict(bytes=file.stat().st_size,sha256=digest)


def copy(source:Path,target:Path,expected:dict|None=None)->dict:
    if source.is_symlink() or not source.is_file():raise ValueError(f"Package input is not a regular file: {source}")
    before=identity(source)
    if expected and any(before[key]!=expected[key] for key in ("bytes","sha256")):
        raise ValueError(f"Package input changed after its original build/source receipt: {source}")
    target.parent.mkdir(parents=True,exist_ok=True)
    if target.exists():raise FileExistsError(f"Preserve an existing package member: {target}")
    with source.open("rb") as input,target.open("xb") as output:shutil.copyfileobj(input,output)
    if identity(target)!=before:raise ValueError("The delivered member differs from its selected input")
    return before


def main()->None:
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scratch-root",type=Path,required=True)
    parser.add_argument("--output",type=Path,required=True)
    args=parser.parse_args()
    scratch=args.scratch_root.resolve(strict=True)
    output=args.output.resolve()
    if not output.is_absolute() or output.exists():raise ValueError("Select a new explicit qualification payload directory")
    source=json.loads((ROOT/"sources.json").read_text(encoding="utf-8"))
    pins=json.loads((ROOT/"ocr-sources.json").read_text(encoding="utf-8"))
    payloads=[]
    cores=[]
    provenance=[]
    def add(original:Path,relative:Path,expected=None):
        record=copy(original,output/relative,expected)
        payloads.append(dict(path=relative.as_posix(),**record))
    for app in source["apps"]:
        engine=app["engine"]
        receipt=json.loads((scratch/f"receipts/{engine}-build.json").read_text(encoding="utf-8-sig"))
        if receipt["buildExit"]!=0 or not receipt["lock"]["pinnedDonorEntriesPreserved"]:
            raise ValueError("A native core lacks its completed original build and donor-lock result")
        current_adapter=identity(ROOT/app["adapter"]["path"])["sha256"]
        if current_adapter!=receipt["input"]["adapterCurrentSha256"]:
            raise ValueError("The current adapter differs from the selected build input")
        executable=Path(f"cores/mailcraft-content-{engine}.exe")
        add(Path(receipt["executable"]),executable,receipt)
        core_source=app["donor"]["sourceUrl"]
        cores.append(dict(id=engine,executable=executable.as_posix(),source=core_source,extensions=EXTENSIONS[engine]))
        donor=scratch/"sources"/engine
        for file in sorted(donor.rglob("*")):
            if not file.is_file():continue
            relative=file.relative_to(donor)
            included=len(relative.parts)==1 or relative.parts[:2] in (("assets","fonts"),("assets","app-icon"))
            if included and file.name.upper().startswith(("LICENSE","NOTICE","COPYING","ATTRIBUTION","ASSETS","OFL")):
                add(file,Path("licences")/engine/relative)
        fonts=app.get("fonts")
        if fonts:
            font_root=scratch/"fonts"/engine
            for notice in fonts["notices"]:
                prefix=f"vendor/Office/licenses/{engine}/craft-fonts/"
                if not notice["path"].startswith(prefix):raise ValueError("Unexpected pinned font licence path")
                relative=Path(notice["path"].removeprefix(prefix))
                file=font_root/relative
                if identity(file)["sha256"]!=notice["sha256"]:raise ValueError("The pinned font licence changed")
                add(file,Path("licences")/engine/"craft-fonts"/relative)
        provenance.append(dict(engine=engine,donor=app["donor"],fonts=fonts,adapterBaseSha256=app["adapter"]["sha256"],
                               adapterCurrentSha256=current_adapter,compatibilityPatches=receipt["input"]["compatibilityPatches"],
                               workerInputs=receipt["input"]["workerInputs"],contractInputs=receipt["contractInputs"]))
    ocr=json.loads((scratch/"receipts/ocr-build.json").read_text(encoding="utf-8-sig"))
    if ocr["buildExit"]!=0 or ocr["version"]!=pins["tesseract"]["version"]:
        raise ValueError("The OCR executable lacks its exact completed source-bound build receipt")
    executable=Path("ocr/tesseract.exe")
    add(Path(ocr["executable"]),executable,ocr)
    for dll in sorted(Path(ocr["executable"]).parent.glob("*.dll")):
        add(dll,Path("ocr")/dll.name)
    if ocr["triplet"]!="mailcraft-x64-explicit":raise ValueError("OCR lacks the explicit static x64 compiler binding")
    installed=scratch/"ocr/installed"/ocr["triplet"]/"share"
    for notice in sorted(installed.rglob("copyright")):
        add(notice,Path("licences/vcpkg")/notice.relative_to(installed))
    for notice in pins["licences"]:
        add(scratch/"ocr/notices"/notice["repository"]/"LICENSE",Path("licences")/notice["repository"]/"LICENSE",notice)
    models=[]
    for model in pins["models"]["files"]:
        relative=Path(model["path"])
        item=copy(scratch/"ocr/models"/relative,output/relative,model)
        models.append(dict(path=relative.as_posix(),**item))
    add(ROOT/"LICENSE-Pathweft",Path("licences/LICENSE-Pathweft"))
    for name in ("LICENSE-MIT","LICENSE-APACHE"):
        add(ROOT.parents[1]/name,Path("licences/MailCraft")/name)
    public=dict(version=1,protocol=1,adapters=provenance,ocr=pins,
                usage="Finite native text/render and local OCR workers; no application server, account settings, credentials or network recognition")
    record=output/"provenance.json"
    record.write_text(json.dumps(public,indent=2)+"\n",encoding="utf-8")
    payloads.append(dict(path="provenance.json",**identity(record)))
    manifest=dict(version=1,payloads=payloads,cores=cores,
                  ocr=dict(executable=executable.as_posix(),version=pins["tesseract"]["version"],
                           languages=[model["language"] for model in pins["models"]["files"]],models=models,
                           source=pins["tesseract"]["release"]))
    (output/"runtime.json").write_text(json.dumps(manifest,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(dict(packaged=True,qualification=False,release=False,output=str(output),cores=len(cores),
                          manifestSha256=identity(output/"runtime.json")["sha256"])))


if __name__=="__main__":main()
