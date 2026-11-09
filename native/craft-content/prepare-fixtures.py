"""Prepare inert, independently described native/OCR email qualification inputs.

No app, CLI, server, account or native engine is launched. Retained Pathweft
fixtures are read-only inputs; outputs belong to exact registered task scratch.
"""
from __future__ import annotations

import argparse
from email.message import EmailMessage
from email import policy
import hashlib
import json
from pathlib import Path
import xml.etree.ElementTree as ET
import zipfile


def sha(data):return hashlib.sha256(data).hexdigest()


def retain(path,data):
    path.parent.mkdir(parents=True,exist_ok=True)
    with path.open("xb") as output:output.write(data)


def office(original,target,engine):
    ns={"w":"http://schemas.openxmlformats.org/wordprocessingml/2006/main",
        "s":"http://schemas.openxmlformats.org/spreadsheetml/2006/main",
        "a":"http://schemas.openxmlformats.org/drawingml/2006/main"}
    markers={"wordcraft":"NativeWordAmber742 Café Καλημέρα", "gridcraft":"SUM(13,29)","deckcraft":"NativeDeckJasper564 Café Καλημέρα"}
    changes=[]
    changed=False
    with zipfile.ZipFile(original) as source,zipfile.ZipFile(target,"x",zipfile.ZIP_DEFLATED) as output:
        for member in source.infolist():
            data=source.read(member)
            selected=(engine=="wordcraft" and member.filename=="word/document.xml") or (engine=="gridcraft" and member.filename.startswith("xl/worksheets/sheet") and member.filename.endswith(".xml")) or (engine=="deckcraft" and member.filename=="ppt/slides/slide1.xml")
            if selected and not changed:
                tree=ET.fromstring(data)
                if engine=="gridcraft":
                    for cell in tree.iter(f"{{{ns['s']}}}c"):
                        formula=cell.find(f"{{{ns['s']}}}f")
                        if formula is None:
                            formula=ET.Element(f"{{{ns['s']}}}f")
                            cell.insert(0,formula)
                        formula.text=markers[engine]
                        cell.attrib.pop("t",None)
                        value=cell.find(f"{{{ns['s']}}}v")
                        if value is None:value=ET.SubElement(cell,f"{{{ns['s']}}}v")
                        value.text="999"
                        changed=True
                        changes.append(dict(part=member.filename,cell=cell.get("r"),formula=markers[engine],cachedValue="999",
                                            reason="Intentionally differs from recalculation; source and cached value must remain distinct"))
                        break
                else:
                    kind="w" if engine=="wordcraft" else "a"
                    text=next(tree.iter(f"{{{ns[kind]}}}t"),None)
                    if text is None:raise ValueError("The retained native text fixture changed structure")
                    text.text=markers[engine]
                    changed=True
                    changes.append(dict(part=member.filename,expectedText=markers[engine]))
                if changed:data=ET.tostring(tree,encoding="utf-8",xml_declaration=True)
            output.writestr(member,data)
    if not changed:raise ValueError(f"The {engine} fixture has no suitable native source location")
    return markers[engine],changes


def pdf():
    content=b"BT /F1 28 Tf 50 160 Td (NativePdfCobalt685) Tj ET\n"
    objects=[b"<< /Type /Catalog /Pages 2 0 R >>",b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
             b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 220] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
             b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
             f"<< /Length {len(content)} >>\nstream\n".encode()+content+b"endstream"]
    result=bytearray(b"%PDF-1.4\n")
    positions=[0]
    for number,value in enumerate(objects,1):
        positions.append(len(result));result.extend(f"{number} 0 obj\n".encode()+value+b"\nendobj\n")
    start=len(result)
    result.extend(f"xref\n0 {len(positions)}\n0000000000 65535 f \n".encode())
    for position in positions[1:]:result.extend(f"{position:010} 00000 n \n".encode())
    result.extend(f"trailer\n<< /Size {len(positions)} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n".encode())
    return bytes(result)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scratch-root",type=Path,required=True)
    parser.add_argument("--registration",type=Path,required=True)
    parser.add_argument("--pathweft",type=Path,required=True)
    parser.add_argument("--attempt",type=int,default=1)
    args=parser.parse_args()
    scratch=args.scratch_root.resolve()
    registration=json.loads(args.registration.read_text(encoding="utf-8"))
    if scratch.parent!=Path("D:/Temp").resolve() or not scratch.name.startswith("mailcraft-index-"):
        raise ValueError("Use exact task-owned native qualification scratch")
    if registration.get("scratchRoot")!=str(scratch) or not registration.get("cleanupTransaction"):
        raise ValueError("Record exact cleanup registration before generating native inputs")
    if args.attempt<1:raise ValueError("Use a positive original preparation attempt")
    root=scratch/("fixtures" if args.attempt==1 else f"fixtures-attempt{args.attempt}")
    root.mkdir(exist_ok=False)
    pathweft=args.pathweft.resolve(strict=True)
    public=pathweft/".local/evidence/2026-11-04-specialists/inputs"
    inventory=json.loads((public/"manifest.json").read_text(encoding="utf-8"))
    records=[]
    for engine,name,mime in (("wordcraft","document-pages.docx","application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
                             ("gridcraft","spreadsheet.xlsx","application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
                             ("deckcraft","navigation-distinct.pptx","application/vnd.openxmlformats-officedocument.presentationml.presentation")):
        record=next(item for item in inventory if item["name"]==name)
        original=public/name
        if sha(original.read_bytes())!=record["sha256"]:raise ValueError("The retained Office source fixture changed")
        target=root/name
        marker,changes=office(original,target,engine)
        records.append(dict(engine=engine,file=name,mime=mime,sha256=sha(target.read_bytes()),expectedNative=marker,
                            original=record,changes=changes))
    for engine,relative,generator in (("vectorcraft","vector-inputs/artboards.vectorcraft","vectorcraft"),
                                      ("figcraft","fig-inputs/frames.figcraft","figcraft"),
                                      ("photocraft","photo-inputs/text.pcraft","photocraft")):
        original=pathweft/".local/evidence/2026-11-06-s16"/relative
        data=original.read_bytes()
        retain(root/original.name,data)
        script=pathweft/f"native/office/{generator}/src/fixtures.rs"
        records.append(dict(engine=engine,file=original.name,mime="application/octet-stream",sha256=sha(data),expectedNative="Located é 子 😀",
                            origin="Retained inert fixture generated by the recorded MIT native source, not a user document",
                            generatorSha256=sha(script.read_bytes())))
    data=pdf();retain(root/"native.pdf",data)
    records.append(dict(engine="pdfcraft",file="native.pdf",mime="application/pdf",sha256=sha(data),expectedNative="NativePdfCobalt685"))
    from PIL import Image,ImageDraw,ImageFont
    raster=Image.new("RGB",(1500,410),"white")
    draw=ImageDraw.Draw(raster)
    font=ImageFont.truetype("C:/Windows/Fonts/segoeui.ttf",58)
    languages=[("eng","LOCAL OCR AMBER 742"),("spa","Factura española café Málaga"),("ell","ΔΟΚΙΜΗ ΚΕΙΜΕΝΟΥ")]
    for index,(_,text) in enumerate(languages):draw.text((45,35+index*115),text,font=font,fill="black")
    raster.save(root/"local-ocr.png")
    records.append(dict(engine="ocr",file="local-ocr.png",mime="image/png",sha256=sha((root/"local-ocr.png").read_bytes()),
                        expectedOcr=[dict(language=language,text=text) for language,text in languages],
                        font="Existing Windows Segoe UI, read-only; no font file is distributed"))
    mbox=bytearray()
    for record in records:
        message=EmailMessage(policy=policy.SMTP)
        message["From"]="Native fixtures <fixtures@example.net>"
        message["To"]="Offline qualification <owner@example.org>"
        message["Date"]="Sun, 08 Nov 2026 12:00:00 +0000"
        message["Message-ID"]=f"<mailcraft-index-{record['engine']}@example.org>"
        message["Subject"]=f"Native content fixture: {record['engine']}"
        message.set_content("Inert offline qualification. The expected search text is in the attachment, not this body.")
        major,minor=record["mime"].split("/",1)
        message.add_attachment((root/record["file"]).read_bytes(),maintype=major,subtype=minor,filename=record["file"])
        mbox.extend(b"From fixtures@example.net Sun Nov  8 12:00:00 2026\n")
        mbox.extend(message.as_bytes().replace(b"\r\n",b"\n"));mbox.extend(b"\n\n")
    retain(root/"fixtures.mbox",bytes(mbox))
    (scratch/"render-evidence").mkdir(exist_ok=False)
    retain(root/"manifest.json",(json.dumps(dict(version=1,sourcePreparation=True,nativeExecution=False,fixtures=records,
        mboxSha256=sha(mbox),privacy="Synthetic/public fixture data only; no account credentials or mail"),indent=2,ensure_ascii=False)+"\n").encode())
    print(json.dumps(dict(sourcePreparation=True,nativeExecution=False,fixtures=len(records),mbox=str(root/"fixtures.mbox"))))


if __name__=="__main__":main()
