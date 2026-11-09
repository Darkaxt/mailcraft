// SPDX-FileCopyrightText: 2026 Protagonist Labs and Pathweft contributors
// SPDX-License-Identifier: MIT
use crate::Raster;
use photocraft_doc::{Document, LayerContent};
use serde_json::{json, Value};

// The donor CPU compositor and real document importers, without its editor,
// GPU renderer, automation or plug-in host. Bounds belong to this preview API.
pub struct Engine { document:Document, notes:Vec<String> }
const PIXELS:u64=64*1024*1024;

impl Engine {
    pub fn open(bytes:&[u8],extension:&str)->Result<Self,String> {
        let (mut document,notes)=match extension {
            "pcraft"=>{
                let limits=photocraft_format::LoadOptions {max_manifest_bytes:8<<20,max_blob_bytes:64<<20,max_total_bytes:256<<20,max_canvas_pixels:PIXELS,..Default::default()};
                let doc=photocraft_format::load_from_bytes_with(bytes,&limits).map_err(|e|e.to_string())?;
                let notes=photocraft_io::open_warnings(&doc);(doc,notes)
            },
            "psd"|"psb"=>{
                if bytes.get(..4)!=Some(b"8BPS") {return Err("Invalid Photoshop signature".into());}
                let dimension=|start|->Result<u32,String>{Ok(u32::from_be_bytes(bytes.get(start..start+4).ok_or("Incomplete Photoshop header")?.try_into().map_err(|_|"Invalid Photoshop dimensions")?))};
                let (height,width)=(dimension(14)?,dimension(18)?);
                if height==0||width==0||u64::from(height)*u64::from(width)>PIXELS {return Err("Photo preview canvas exceeds 64 megapixels".into());}
                let options=photocraft_io::ImportOptions {psd_max_pixel_bytes:256<<20,..Default::default()};
                let imported=photocraft_io::import_with_options(&format!("document.{extension}"),bytes,&options).map_err(|e|e.to_string())?;
                (imported.document,imported.warnings)
            },
            _=>return Err("Unsupported PhotoCraft input".into()),
        };
        if document.size.is_empty()||document.size.area()>PIXELS {return Err("Photo preview canvas exceeds 64 megapixels".into());}
        if document.max_group_depth()>photocraft_format::MAX_GROUP_DEPTH {return Err("Photo preview layer nesting exceeds its limit".into());}
        // Native files can contain editable type without a saved pixel cache.
        // Use the donor text painter, with its import-side ink allocation bound.
        let dpi=document.resolution_dpi;let format=document.pixel_format();
        let mut stack=vec![document.layers.as_mut_slice()];
        while let Some(layers)=stack.pop() {
            for layer in layers {
                match &mut layer.content {
                    LayerContent::Group(group)=>stack.push(group.children.as_mut_slice()),
                    LayerContent::Text(text) if text.cache.is_none()=>{
                        let mut painter=photocraft_text::shared().lock().unwrap_or_else(|e|e.into_inner());
                        let layout=painter.layout(text,dpi);
                        let warp=photocraft_text::render::layout_deform(&layout,text);
                        let ink=photocraft_text::render::ink_rect_warped(&layout,&text.transform,warp.as_ref());
                        if u64::from(ink.width())*u64::from(ink.height())>PIXELS {return Err("Photo text raster exceeds the preview limit".into());}
                        painter.render_layer(text,dpi,format);
                    },_=>{},
                }
            }
        }
        Ok(Self{document,notes})
    }

    pub fn info(&self)->Value {
        json!({"engine":"PhotoCraft","kind":"image","parts":[{"name":"Composite","pages":1}],"notes":self.notes})
    }

    pub fn extract(&self,unit:u64,offset:usize,limit:usize)->Result<Value,String> {
        let mut stack=vec![(self.document.layers.as_slice(),Vec::<usize>::new(),true)];
        let units=std::iter::from_fn(move ||{
            while let Some((layers,path,parent_visible))=stack.pop() {
                if let Some((layer,rest))=layers.split_first() {
                    let index=path.last().copied().unwrap_or(0);
                    let mut following=path.clone();if let Some(last)=following.last_mut(){*last+=1;}else{following.push(1);}
                    stack.push((rest,following,parent_visible));
                    let mut current=path;if current.is_empty(){current.push(index);}
                    let visible=parent_visible&&layer.visible;
                    if let LayerContent::Group(group)=&layer.content {let mut child=current.clone();child.push(0);stack.push((&group.children,child,visible));}
                    if let LayerContent::Text(text)=&layer.content {
                        return Some((json!({"layerPath":current,"layerId":layer.id.0,"layerName":layer.name,"visible":visible,"kind":"textLayer"}),move ||text.text.clone()));
                    }
                }
            }
            None
        });
        let mut batch=crate::extraction::page(units,unit,offset,limit,"Imported editable text layers in bottom-to-top layer-tree order, including hidden layers marked visible=false; no smart-object recursion, flattened pixels or OCR")?;
        if batch["status"]=="empty" {batch["status"]=json!("image-only");}
        Ok(batch)
    }

    pub fn render(&mut self,part:u32,page:u32,width:u32)->Result<Raster,String> {
        if part!=0||page!=0 {return Err("Invalid photo composite".into());}
        let mut buffer=photocraft_compose::thumbnail_buffer(&self.document,width.min(4096));
        let profile=photocraft_io::pcraft_full_composite_icc(&self.document);
        let source=photocraft_cms::Profile::parse(&profile).map_err(|e|e.to_string())?;
        let target=photocraft_cms::Builtin::Srgb.profile();
        if !source.same_colors(target) {
            let transform=photocraft_cms::Transform::new(&source,target,photocraft_cms::Intent::RelativeColorimetric,true).map_err(|e|e.to_string())?;
            transform.apply(buffer.px.as_flattened_mut(),4);
        }
        let mut image=buffer.to_rgba8();
        for pixel in image.pixels.chunks_exact_mut(4) {let alpha=u16::from(pixel[3]);for channel in &mut pixel[..3]{*channel=((u16::from(*channel)*alpha+127)/255) as u8;}}
        Ok(Raster{width:image.width,height:image.height,format:0,pixels:image.pixels})
    }
}
