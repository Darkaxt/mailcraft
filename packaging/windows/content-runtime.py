"""Bind the qualified Windows x64 runtime to portable and MSI packaging.

Only source/payload files are read or copied. No native executable is launched.
The immutable selection is version-controlled; changing a source input requires
a new runtime assembly and qualification, not silently shipping stale workers.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path, PurePosixPath
import uuid
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
NAMESPACE = "http://wixtoolset.org/schemas/v4/wxs"
COMPONENT_NAMESPACE = uuid.UUID("e04ca23f-44cc-4d21-b3aa-38adf01f4722")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def identifier(kind, value):
    return kind + hashlib.sha256(value.encode("utf-8")).hexdigest()[:32]


def selected(source):
    spec = importlib.util.spec_from_file_location("mailcraft_copy_runtime", ROOT / "native/craft-content/copy-runtime.py")
    copier = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(copier)
    copier.plain(source)
    lock = json.loads((ROOT / "native/craft-content/release-runtime.json").read_text(encoding="utf-8"))
    if lock["version"] != 1 or lock["platform"] != "windows-x64":
        raise ValueError("Unsupported release runtime selection")
    for name, expected in lock["sourceFiles"].items():
        path = ROOT / name
        copier.plain(path)
        if digest(path) != expected:
            raise ValueError(f"Requalify the runtime after changing its source: {name}")
    if digest(source / "runtime.json") != lock["manifestSha256"]:
        raise ValueError("The runtime is not the independently selected qualified manifest")
    return copier


def fragment(runtime, destination):
    if destination.exists():
        raise FileExistsError("Preserve the existing MSI runtime fragment")
    ET.register_namespace("", NAMESPACE)
    tag = lambda value: f"{{{NAMESPACE}}}{value}"
    document = ET.Element(tag("Wix"))
    fragment = ET.SubElement(document, tag("Fragment"))
    group = ET.SubElement(fragment, tag("ComponentGroup"), {"Id": "MailcraftContentRuntime"})
    if runtime is not None:
        directory = ET.SubElement(fragment, tag("DirectoryRef"), {"Id": "INSTALLFOLDER"})
        directories = {PurePosixPath("."): ET.SubElement(directory, tag("Directory"), {"Id": "CraftContentRoot", "Name": "craft-content"})}
        for path in sorted(runtime.rglob("*")):
            if not path.is_file():
                continue
            relative = PurePosixPath(path.relative_to(runtime).as_posix())
            parent = PurePosixPath(".")
            for part in relative.parts[:-1]:
                child = parent / part
                if child not in directories:
                    directories[child] = ET.SubElement(directories[parent], tag("Directory"), {"Id": identifier("RuntimeDir", child.as_posix()), "Name": part})
                parent = child
            directory_id = directories[parent].attrib["Id"]
            component = ET.SubElement(group, tag("Component"), {"Id": identifier("RuntimeComponent", relative.as_posix()), "Directory": directory_id,
                "Guid": str(uuid.uuid5(COMPONENT_NAMESPACE, "craft-content/" + relative.as_posix()))})
            ET.SubElement(component, tag("File"), {"Id": identifier("RuntimeFile", relative.as_posix()), "Source": str(path), "Name": relative.name, "KeyPath": "yes"})
    ET.indent(document)
    with destination.open("xb") as stream:
        stream.write(ET.tostring(document, encoding="utf-8", xml_declaration=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--stage", type=Path, required=True)
    parser.add_argument("--fragment", type=Path, required=True)
    parser.add_argument("--arch", choices=("x64", "x86", "arm64"), required=True)
    args = parser.parse_args()
    runtime = None
    if args.arch == "x64":
        if args.source is None:
            raise ValueError("A Windows x64 release must include the selected seven-core/OCR runtime")
        copier = selected(args.source)
        runtime = args.stage / "craft-content"
        copier.copy_runtime(args.source, runtime)
    elif args.source is not None:
        raise ValueError("The qualified x64 runtime cannot be labelled as another architecture")
    fragment(runtime, args.fragment)
    print(json.dumps({"runtimePackaged": runtime is not None, "nativeLaunched": False, "fragment": str(args.fragment)}))


if __name__ == "__main__":
    main()
