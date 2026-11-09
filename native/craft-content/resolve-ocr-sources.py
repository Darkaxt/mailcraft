"""Pin official OCR sources and model Git objects, without running native code.

The small output is a retained reproducibility input. Archive/model bytes are
streamed only for source identity; this command creates no build/download tree.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from urllib.request import Request, urlopen


HEADERS = {"User-Agent":"MailCraft-source-preparation", "Accept":"application/vnd.github+json"}


def request(url: str):
    if not url.startswith(("https://api.github.com/repos/tesseract-ocr/", "https://raw.githubusercontent.com/tesseract-ocr/",
                           "https://codeload.github.com/tesseract-ocr/")):
        raise ValueError("OCR provenance must remain in the official publisher repositories")
    return urlopen(Request(url, headers=HEADERS))


def metadata(path: str):
    with request("https://api.github.com/repos/tesseract-ocr/"+path) as response:
        data=response.read(4*1024*1024+1)
    if len(data)>4*1024*1024:
        raise ValueError("Narrow the official OCR metadata request")
    return json.loads(data)


def fingerprint(url: str, maximum: int, git_sha: str | None = None, git_bytes: int | None = None) -> dict:
    sha256,sha512,git=hashlib.sha256(),hashlib.sha512(),hashlib.sha1()
    if git_sha:
        if git_bytes is None:
            raise ValueError("A Git blob must have its declared byte length")
        git.update(f"blob {git_bytes}\0".encode())
    total=0
    with request(url) as response:
        while chunk:=response.read(64*1024):
            total+=len(chunk)
            if total>maximum:
                raise ValueError("OCR source exceeded its declared preparation allowance")
            sha256.update(chunk)
            sha512.update(chunk)
            git.update(chunk)
    if git_sha and (total!=git_bytes or git.hexdigest()!=git_sha):
        raise ValueError("The official source bytes differ from the independently read Git object")
    return dict(url=url, bytes=total,sha256=sha256.hexdigest(),sha512=sha512.hexdigest(),gitBlob=git_sha)


def main() -> None:
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version",required=True)
    parser.add_argument("--output",required=True,type=Path)
    args=parser.parse_args()
    if args.output.exists():
        raise FileExistsError("Preserve existing OCR pins; an intentional update must be reviewable")
    release=metadata(f"tesseract/releases/tags/{args.version}")
    if release.get("draft") or release.get("prerelease") or release.get("tag_name")!=args.version:
        raise ValueError("Select an exact published stable official OCR source release")
    ref=metadata(f"tesseract/git/ref/tags/{args.version}")["object"]
    while ref["type"]=="tag":
        ref=metadata(f"tesseract/git/tags/{ref['sha']}")["object"]
    if ref["type"]!="commit":
        raise ValueError("The official release tag does not identify a source commit")
    commit=ref["sha"]
    archive=fingerprint(f"https://codeload.github.com/tesseract-ocr/tesseract/tar.gz/{commit}",64*1024*1024)
    model_repo=metadata("tessdata_fast")
    model_commit=metadata(f"tessdata_fast/commits/{model_repo['default_branch']}")["sha"]
    tree=metadata(f"tessdata_fast/git/trees/{model_commit}")
    if tree.get("truncated"):
        raise ValueError("The official model tree was truncated")
    entries={entry["path"]:entry for entry in tree["tree"]}
    models=[]
    for language in ("eng","spa","ell"):
        name=f"{language}.traineddata"
        entry=entries[name]
        if entry["type"]!="blob":
            raise ValueError("The selected OCR model is not an official file")
        record=fingerprint(f"https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/{model_commit}/{name}",16*1024*1024,entry["sha"],entry["size"])
        models.append(dict(language=language,path=f"tessdata/{name}",**record))
    notices=[]
    for repo,revision in (("tesseract",commit),("tessdata_fast",model_commit)):
        notice=fingerprint(f"https://raw.githubusercontent.com/tesseract-ocr/{repo}/{revision}/LICENSE",1024*1024)
        notices.append(dict(repository=repo,**notice))
    record=dict(version=1,sourcePreparation=True,nativeExecution=False,
                tesseract=dict(repository="https://github.com/tesseract-ocr/tesseract",version=args.version,
                               commit=commit,release=release["html_url"],archive=archive),
                models=dict(repository="https://github.com/tesseract-ocr/tessdata_fast",commit=model_commit,files=models),
                licences=notices,networkPolicy="Source acquisition only; recognition remains local with pinned models")
    with args.output.open("x",encoding="utf-8",newline="\n") as output:
        json.dump(record,output,indent=2)
        output.write("\n")
    print(json.dumps(dict(sourcePreparation=True,nativeExecution=False,version=args.version,sourceCommit=commit,
                          modelCommit=model_commit,languages=[model["language"] for model in models])))


if __name__=="__main__":
    main()
