// SPDX-FileCopyrightText: 2026 Protagonist Labs and Pathweft contributors
// SPDX-License-Identifier: MIT
use crate::Raster;
use serde_json::{json,Value};
use wordcraft_doc::{Document,Block,Blocks};
use wordcraft_layout::{DocLayout,LayoutCache,LayoutOptions};

pub struct Engine { document: Document, layout: DocLayout }

impl Engine {
    pub fn open(bytes:&[u8],extension:&str)->Result<Self,String> {
        let document=match extension {
            "docx"|"docm"=>wordcraft_docx::read(bytes).map_err(|e|e.to_string())?,
            "doc"=>wordcraft_docbin::read(bytes).map_err(|e|e.to_string())?,
            "odt"=>wordcraft_formats::import(extension,bytes).ok_or("Unsupported Word format")??,
            _=>return Err("Unsupported Word format".into()),
        };
        let layout=wordcraft_layout::layout(&document,&mut LayoutCache::new(),&LayoutOptions {hide_deleted:true,..Default::default()});
        if layout.pages.is_empty() {return Err("Craft produced no document pages".into());}
        Ok(Self {document,layout})
    }

    pub fn info(&self)->Value {
        json!({"engine":"WordCraft","kind":"document","parts":[{"name":"Document","pages":self.layout.pages.len()}],"notes":[]})
    }

    pub fn extract(&self,unit:u64,offset:usize,limit:usize)->Result<Value,String> {
        let stories=std::iter::once((json!({"story":"body"}),&self.document.body))
            .chain(self.document.parts.iter().map(|(id,part)|(json!({"story":"part","part":id,"kind":format!("{:?}",part.kind)}),&part.blocks)));
        let units=stories.flat_map(|(story,blocks)|Paragraphs::new(blocks).map(move |(path,text)|{
            let mut location=story.clone();location["paragraph"]=json!(path);(location,move ||text.final_text())
        }));
        crate::extraction::page(units,unit,offset,limit,"Imported final paragraphs and table cells in body and document parts; no OCR, drawings or unsupported import content")
    }

    pub fn render(&mut self,part:u32,page:u32,width:u32)->Result<Raster,String> {
        if part!=0 {return Err("Invalid document part".into());}
        let page=self.layout.pages.get(page as usize).ok_or("Invalid document page")?;
        if !page.w.is_finite() || !page.h.is_finite() || page.w<=0.0 || page.h<=0.0 {return Err("Invalid document page geometry".into());}
        // Limit both axes while preserving the donor's aspect ratio and layout.
        let scale=(width as f32/page.w).min(4096.0/page.h);
        let raster=wordcraft_render::render_page(&self.document,page,scale,&Default::default());
        Ok(Raster {width:raster.width,height:raster.height,format:0,pixels:raster.pixels})
    }
}

// An explicit traversal stack avoids both recursive call-stack growth and a
// retained list of every paragraph/text string. Paths match donor block paths.
enum Step<'a> {Blocks(&'a Blocks,usize,Vec<usize>),Cells(&'a wordcraft_doc::Table,usize,usize,Vec<usize>)}
struct Paragraphs<'a> {stack:Vec<Step<'a>>}
impl<'a> Paragraphs<'a> {fn new(blocks:&'a Blocks)->Self {Self {stack:vec![Step::Blocks(blocks,0,vec![])]}}}
impl<'a> Iterator for Paragraphs<'a> {
    type Item=(Vec<usize>,&'a wordcraft_doc::Paragraph);
    fn next(&mut self)->Option<Self::Item> {
        while let Some(step)=self.stack.pop() {
            match step {
                Step::Blocks(blocks,index,prefix)=>{
                    let Some(block)=blocks.get(index) else {continue};
                    self.stack.push(Step::Blocks(blocks,index+1,prefix.clone()));
                    let mut path=prefix;path.push(index);
                    match block.as_ref() {
                        Block::Para(p)=>return Some((path,p)),
                        Block::Table(table)=>self.stack.push(Step::Cells(table,0,0,path)),
                    }
                },
                Step::Cells(table,row,cell,prefix)=>{
                    let Some(r)=table.rows.get(row) else {continue};
                    if let Some(c)=r.cells.get(cell) {
                        self.stack.push(Step::Cells(table,row,cell+1,prefix.clone()));
                        let mut path=prefix;path.extend([row,cell]);
                        self.stack.push(Step::Blocks(&c.blocks,0,path));
                    }else{self.stack.push(Step::Cells(table,row+1,0,prefix));}
                }
            }
        }
        None
    }
}
