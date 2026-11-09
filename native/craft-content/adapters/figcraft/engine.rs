// SPDX-FileCopyrightText: 2026 Protagonist Labs and Pathweft contributors
// SPDX-License-Identifier: MIT
use crate::Raster;
use figcraft_doc::{Document,NodeId,NodeKind,geom::Rect};
use serde_json::{json,Value};

struct Part {name:String,ids:Vec<NodeId>,bounds:Rect}
pub struct Engine {document:Document,parts:Vec<Part>,notes:Vec<String>}
impl Engine {
    pub fn open(bytes:&[u8],extension:&str)->Result<Self,String> {
        let (document,mut notes)=match extension {
            "figcraft"=>figcraft_format::load_bytes(bytes).map_err(|e|e.to_string())?,
            "fig"=>{
                let (doc,mut notes)=figcraft_format::fig::import(bytes,"Imported design").map_err(|e|e.to_string())?;
                notes.push("Best-effort Figma import; unsupported properties may be skipped or approximated".into());(doc,notes)
            },
            _=>return Err("Unsupported FigCraft input".into()),
        };
        let mut parts=Vec::new();
        for &page in &document.pages {
            let node=document.node(page).ok_or("Invalid design page")?;
            let bounds=document.page_content_bounds(page).unwrap_or(Rect::new(0.0,0.0,100.0,100.0));
            parts.push(Part{name:format!("{} / Overview",node.name),ids:node.children.clone(),bounds});
            for &id in &node.children {
                let child=document.node(id).ok_or("Invalid design child")?;
                if child.visible&&matches!(&child.kind,NodeKind::Frame|NodeKind::Component{..}|NodeKind::Section{..}) {
                    if let Some(bounds)=figcraft_render::export_bounds(&document,id) {parts.push(Part{name:format!("{} / {}",node.name,child.name),ids:vec![id],bounds});}
                }
            }
        }
        if parts.is_empty()||parts.len()>1000 {return Err("Design preview requires between 1 and 1000 page/frame views".into());}
        for part in &parts {let r=part.bounds;if ![r.x0,r.y0,r.x1,r.y1].into_iter().all(f64::is_finite)||r.width()<=0.0||r.height()<=0.0 {return Err("Invalid design frame bounds".into());}}
        // The native importer carries repairs; Figma import carries its own
        // approximation notes. Do not imply text completeness from rendering.
        notes.retain(|note|!note.is_empty());
        Ok(Self{document,parts,notes})
    }
    pub fn info(&self)->Value {
        json!({"engine":"FigCraft","kind":"frames","parts":self.parts.iter().map(|p|json!({"name":p.name,"pages":1})).collect::<Vec<_>>(),"notes":self.notes})
    }
    pub fn render(&mut self,part:u32,page:u32,width:u32)->Result<Raster,String> {
        if page!=0 {return Err("Invalid frame page".into());}
        let part=self.parts.get(part as usize).ok_or("Invalid design frame")?;
        let scale=f64::from(width.min(4096))/part.bounds.width().max(part.bounds.height());
        let pixels=figcraft_render::render_area(&self.document,&part.ids,part.bounds,scale).ok_or("Design renderer could not allocate the bounded frame")?;
        Ok(Raster{width:pixels.width(),height:pixels.height(),format:0,pixels:pixels.data().to_vec()})
    }
    pub fn extract(&self,unit:u64,offset:usize,limit:usize)->Result<Value,String> {
        let mut stack=self.document.pages.iter().enumerate().rev().map(|(page,&id)|(id,page,Vec::<usize>::new(),true)).collect::<Vec<_>>();
        let units=std::iter::from_fn(move || {
            while let Some((id,page,path,parent_visible))=stack.pop() {
                let node=self.document.node(id)?;let visible=parent_visible&&node.visible;
                for (index,&child) in node.children.iter().enumerate().rev() {let mut next=path.clone();next.push(index);stack.push((child,page,next,visible));}
                if let NodeKind::Text(text)=&node.kind {
                    return Some((json!({"page":page,"nodePath":path,"nodeId":node.id,"nodeName":node.name,"visible":visible,"transform":self.document.world_transform(id).as_coeffs(),"kind":"textNode"}),move ||text.characters.clone()));
                }
            }
            None
        });
        crate::extraction::page(units,unit,offset,limit,"Imported editable text nodes in page/tree order, including hidden nodes marked visible=false; native document transforms; no outlined glyph recognition, comments, version history or OCR")
    }
}
