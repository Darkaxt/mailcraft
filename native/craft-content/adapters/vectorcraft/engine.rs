// SPDX-FileCopyrightText: 2026 Protagonist Labs and Pathweft contributors
// SPDX-License-Identifier: MIT
use crate::Raster;
use serde_json::{json,Value};
use vectorcraft_doc::{Document,NodeKind};
use vectorcraft_render::{Renderer,RenderOptions};

pub struct Engine { document:Document, renderer:Renderer, notes:Vec<String> }
impl Engine {
    pub fn open(bytes:&[u8],extension:&str)->Result<Self,String> {
        let (document,mut notes)=match extension {
            "vectorcraft"|"drawcraft"=>{
                let native=vectorcraft_format::load_file(bytes).map_err(|e|e.to_string())?;
                let mut notes=native.depth_notes;
                for (name,icc) in native.profiles.into_iter().take(8) {
                    if vectorcraft_color::cms::profile(&name).is_none() {
                        if let Err(error)=vectorcraft_color::cms::register_icc(&icc,Some(name.clone())) {notes.push(format!("Embedded colour profile {name}: {error}"));}
                    }
                }
                (native.doc,notes)
            },
            "svg"|"svgz"=>{
                let text=vectorcraft_svg::text_of(bytes).map_err(|e|e.to_string())?;
                vectorcraft_svg::import_with_report(&text).map_err(|e|e.to_string())?
            },
            "ai"=>{
                if vectorcraft_pdf::is_postscript(bytes)||!bytes.starts_with(b"%PDF-") {return Err("This preview supports PDF-compatible AI, not legacy PostScript AI".into());}
                if vectorcraft_pdf::info(bytes,None).map_err(|e|e.to_string())?.pages.len()>1000 {return Err("Vector preview exceeds 1000 PDF artboards".into());}
                let options=vectorcraft_pdf::ImportOptions {max_pages:Some(1000),uncalibrated_rgb:true,..Default::default()};
                let imported=vectorcraft_pdf::import_with_report(bytes,&options).map_err(|e|e.to_string())?;
                let mut notes=imported.warnings;notes.push("PDF-compatible artwork only; Illustrator private editing data is not interpreted".into());
                (imported.document,notes)
            },
            _=>return Err("Unsupported VectorCraft input".into()),
        };
        if document.artboards.is_empty()||document.artboards.len()>1000 {return Err("Vector preview requires between 1 and 1000 artboards".into());}
        for board in &document.artboards {
            let r=board.rect;
            if ![r.x0,r.y0,r.x1,r.y1].into_iter().all(f64::is_finite)||r.width()<=0.0||r.height()<=0.0 {return Err("Invalid vector artboard bounds".into());}
        }
        let mut linked=false;let mut plugins=false;let mut stack=document.layers.iter().collect::<Vec<_>>();
        while let Some(node)=stack.pop() {
            linked|=match &node.kind {NodeKind::Image(image)=>image.link.is_some(),NodeKind::PlacedDocument(_)=>true,_=>false};
            plugins|=vectorcraft_render::effects::has_plugin_effect(node);
            if let Some(children)=node.children(){stack.extend(children);}
        }
        if linked {notes.push("Linked artwork uses embedded data or saved previews; external files are not fetched".into());}
        if plugins {notes.push("This document uses plug-in effects; installed plug-ins are not loaded in previews".into());}
        Ok(Self{document,renderer:Renderer::new(),notes})
    }
    pub fn info(&self)->Value {
        json!({"engine":"VectorCraft","kind":"artboards","parts":self.document.artboards.iter().map(|a|json!({"name":a.name,"pages":1})).collect::<Vec<_>>(),"notes":self.notes})
    }
    pub fn render(&mut self,part:u32,page:u32,width:u32)->Result<Raster,String> {
        if page!=0 {return Err("Invalid artboard page".into());}
        let region=self.document.artboards.get(part as usize).ok_or("Invalid artboard")?.rect;
        let scale=f64::from(width.min(4096))/region.width().max(region.height());
        let options=RenderOptions {background:Some([255;4]),skip_templates:true,hide_guides:true,highlight_substituted_fonts:false,..Default::default()};
        let image=self.renderer.render_region_with(&self.document,region,scale,&options);
        Ok(Raster{width:image.width,height:image.height,format:0,pixels:image.pixels})
    }
    pub fn extract(&self,unit:u64,offset:usize,limit:usize)->Result<Value,String> {
        let mut stack=vec![(self.document.layers.as_slice(),Vec::<usize>::new(),true)];
        let units=std::iter::from_fn(move || {
            while let Some((nodes,path,parent_visible))=stack.pop() {
                if let Some((node,rest))=nodes.split_first() {
                    let mut following=path.clone();if let Some(last)=following.last_mut(){*last+=1;}else{following.push(1);}
                    stack.push((rest,following,parent_visible));
                    let mut current=path;if current.is_empty(){current.push(0);}
                    let visible=parent_visible&&node.visible;
                    if let Some(children)=node.children() {let mut child=current.clone();child.push(0);stack.push((children.as_slice(),child,visible));}
                    if let NodeKind::Text(text)=&node.kind {
                        return Some((json!({"objectPath":current,"objectId":node.id,"objectName":node.name,"visible":visible,"transform":text.xf.as_coeffs(),"kind":"textObject"}),move ||text.plain_text()));
                    }
                }
            }
            None
        });
        crate::extraction::page(units,unit,offset,limit,"Imported editable text objects in layer-tree order; document coordinates, including hidden objects marked visible=false; no outline recognition, symbol-instance expansion, linked documents or OCR")
    }
}
