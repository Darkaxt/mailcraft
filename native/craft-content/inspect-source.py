"""Inspect a bounded API question in one exact pinned donor source archive."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", required=True)
    parser.add_argument("--archives", type=Path, required=True)
    parser.add_argument("--path", required=True)
    parser.add_argument("--pattern", required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent
    manifest = json.loads((root / "sources.json").read_text(encoding="utf-8"))
    matches = [app for app in manifest["apps"] if app["engine"] == args.engine]
    if len(matches) != 1:
        raise ValueError("Select one exact recorded donor")
    donor = matches[0]["donor"]
    archive = args.archives / donor["sourceArchive"]
    with archive.open("rb") as source:
        if hashlib.file_digest(source, "sha256").hexdigest() != donor["sourceSha256"]:
            raise ValueError("The donor archive differs from its pinned source")
    printed = 0
    with zipfile.ZipFile(archive) as source:
        for member in source.infolist():
            if member.is_dir() or not re.search(args.path, member.filename):
                continue
            if member.file_size > 2 * 1024 * 1024:
                raise ValueError("Narrow the API inspection to a regular source file")
            lines = source.read(member).decode("utf-8").splitlines()
            for number, line in enumerate(lines):
                if not re.search(args.pattern, line):
                    continue
                print(json.dumps(dict(path=member.filename, line=number + 1,
                    excerpt=lines[max(0, number - 1):number + 5]), ensure_ascii=False))
                printed += 1
                if printed >= 16:
                    print(json.dumps(dict(diagnosticLimit=16, narrowPatternForMore=True)))
                    return


if __name__ == "__main__":
    main()
