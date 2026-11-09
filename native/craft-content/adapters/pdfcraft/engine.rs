// SPDX-FileCopyrightText: 2026 Protagonist Labs and Pathweft contributors
// SPDX-License-Identifier: MIT
use crate::Raster;
use std::{cell::RefCell,sync::Arc};
use pdfcraft_render::{PageRenderer,RenderConfig,RenderRequest,RequestKind,PageText};
use serde_json::{json,Value};

struct PageCache {page:usize,text:Arc<PageText>,lines:Vec<std::ops::Range<usize>>}
pub struct Engine {
    renderer:RefCell<PageRenderer>,
    pages:Vec<(f32,f32)>,
    image_resources:Vec<bool>,
    notes:Vec<String>,
    text:RefCell<Option<PageCache>>,
}
impl Engine {
    pub fn open(bytes:&[u8],extension:&str)->Result<Self,String> {
        if extension!="pdf" {return Err("Unsupported PDF input".into());}
        let bytes=Arc::new(bytes.to_vec());
        let info=pdfcraft_render::inspect(bytes.clone(),None).map_err(|e|e.to_string())?;
        if info.pages.is_empty()||info.pages.len()>100000 {return Err("PDF page count is outside preview limits".into());}
        let pages=info.pages.iter().map(|p|(p.width,p.height)).collect::<Vec<_>>();
        if pages.iter().any(|&(w,h)|!w.is_finite()||!h.is_finite()||w<=0.0||h<=0.0) {return Err("Invalid PDF page geometry".into());}
        // Inspect resource dictionaries only; do not decode images, launch actions
        // or load linked files. This identifies image resources, not OCR content.
        let image_resources=pdfcraft_cos::Document::open(bytes.clone()).ok().map(|doc| {
            pdfcraft_model::pages(&doc).iter().map(|page|page.dict.get(b"Resources").is_some_and(|r|has_image(&doc,r,0,&mut 10000))).collect()
        }).unwrap_or_default();
        let renderer=PageRenderer::new(bytes,RenderConfig::default());
        if renderer.page_count()!=pages.len(){return Err("PDF parser page counts differ".into());}
        Ok(Self{renderer:RefCell::new(renderer),pages,image_resources,notes:info.warnings,text:RefCell::new(None)})
    }
    pub fn info(&self)->Value {
        json!({"engine":"PdfCraft","kind":"document","parts":[{"name":"Document","pages":self.pages.len()}],"notes":self.notes})
    }
    pub fn render(&mut self,part:u32,page:u32,width:u32)->Result<Raster,String> {
        if part!=0 {return Err("Invalid PDF document part".into());}
        let &(w,h)=self.pages.get(page as usize).ok_or("Invalid PDF page")?;
        let result=self.renderer.borrow_mut().render(RenderRequest{page:page as usize,scale:width.min(4096) as f32/w.max(h),..Default::default()});
        if let Some(error)=result.error{return Err(error);}
        if !result.warnings.is_empty(){return Err(format!("PDF rendering is incomplete: {:?}",result.warnings));}
        Ok(Raster{width:result.width,height:result.height,format:0,pixels:result.rgba.to_vec()})
    }
    pub fn extract(&self,unit:u64,offset:usize,limit:usize)->Result<Value,String> {
        let page=(unit>>32) as usize;let line=(unit&0xffffffff) as usize;
        if page>=self.pages.len(){return Err("Invalid PDF text cursor".into());}
        let mut cache=self.text.borrow_mut();
        if cache.as_ref().is_none_or(|c|c.page!=page) {
            let result=self.renderer.borrow_mut().render(RenderRequest{page,kind:RequestKind::Text,..Default::default()});
            if let Some(error)=result.error{return Err(error);}
            if !result.warnings.is_empty(){return Err(format!("PDF text interpretation is incomplete: {:?}",result.warnings));}
            let text=result.text.ok_or("PDF text interpreter returned no result")?;
            let mut lines=Vec::new();let mut start=0;
            while start<text.glyphs.len() {
                let index=text.line_of[start];let mut end=start+1;
                while end<text.glyphs.len()&&text.line_of[end]==index {end+=1;}
                lines.push(start..end);start=end;
            }
            if lines.len()>u32::MAX as usize {return Err("PDF line count exceeds cursor limits".into());}
            *cache=Some(PageCache{page,text,lines});
        }
        let current=cache.as_ref().ok_or("Missing PDF text page")?;
        let units=current.lines.iter().enumerate().map(|(index,range)| {
            let rects=current.text.line_rects(range.clone());
            (json!({"page":page,"line":index,"glyphStart":range.start,"glyphEnd":range.end,"rects":rects,"space":"page-view points, top-left origin, y down"}),||current.text.text_of(range.clone()))
        });
        let mut result=crate::extraction::page(units,line as u64,offset,limit,"Interpreted PDF text in imported reading order, page/line/glyph ranges and page-view rectangles; ToUnicode/encoding fallbacks; no OCR, scripts or linked-resource loading")?;
        if !result["next"].is_null() {
            let next=result["next"]["unit"].as_u64().ok_or("Invalid PDF line cursor")?;
            result["next"]["unit"]=json!(((page as u64)<<32)|next);
        }else if page+1<self.pages.len(){result["next"]=json!({"unit":((page+1) as u64)<<32,"offset":0});}
        if current.text.is_empty()&&self.image_resources.get(page)==Some(&true){result["status"]=json!("image-only");}
        Ok(result)
    }
}

fn has_image(doc:&pdfcraft_cos::Document,resources:&pdfcraft_cos::Object,depth:usize,budget:&mut usize)->bool {
    if depth>16||*budget==0{return false;}*budget-=1;
    let resolved=doc.resolve(resources);
    let Some(dict)=resolved.as_dict() else{return false;};
    let Some(objects)=dict.get(b"XObject") else{return false;};
    let objects=doc.resolve(objects);let Some(objects)=objects.as_dict() else{return false;};
    for (_,object) in objects.iter() {
        if *budget==0 {break;}*budget-=1;
        let object=doc.resolve(object);let Some(dict)=object.as_dict() else{continue;};
        if dict.name(b"Subtype")==Some(b"Image"){return true;}
        if dict.name(b"Subtype")==Some(b"Form")&&dict.get(b"Resources").is_some_and(|r|has_image(doc,r,depth+1,budget)){return true;}
    }
    false
}
