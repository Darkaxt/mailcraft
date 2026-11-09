// SPDX-FileCopyrightText: 2026 Protagonist Labs and Pathweft contributors
// SPDX-License-Identifier: MIT
use crate::Raster;
use serde_json::{json,Value};
use deckcraft_model::{Presentation,Shape,ShapeKind};
use deckcraft_render::RenderOpts;

pub struct Engine { presentation:Presentation, notes:Vec<String> }

impl Engine {
    pub fn open(bytes:&[u8],extension:&str)->Result<Self,String> {
        if extension!="pptx" {return Err("Unsupported presentation format".into());}
        let (presentation,skipped)=deckcraft_pptx::import_checked(bytes).map_err(|e|e.to_string())?;
        if presentation.slides.is_empty() {return Err("Craft produced no slides".into());}
        let notes=skipped.into_iter().map(|part|format!("Could not import slide part: {part}")).collect();
        Ok(Self {presentation,notes})
    }

    pub fn info(&self)->Value {
        json!({"engine":"DeckCraft","kind":"presentation","parts":[{"name":"Slides","pages":self.presentation.slides.len()}],"notes":self.notes})
    }

    pub fn extract(&self,unit:u64,offset:usize,limit:usize)->Result<Value,String> {
        let units=self.presentation.slides.iter().enumerate().flat_map(|(slide_index,slide)|{
            let shapes=Shapes::new(&slide.shapes).flat_map(move |(path,shape)|{
                let text=shape.text.as_ref().into_iter().flat_map(|body|body.paragraphs.iter().enumerate())
                    .map({let path=path.clone();move |(paragraph,p)|(json!({"slide":slide_index,"shape":path,"shapeId":shape.id.0,"paragraph":paragraph,"kind":"shape"}),Box::new(move ||p.text()) as Box<dyn FnOnce()->String>)});
                let table=match &shape.kind {ShapeKind::Table(table)=>Some(table),_=>None};
                let cells=table.into_iter().flat_map(|t|t.rows.iter().enumerate()).flat_map(|(row,r)|r.cells.iter().enumerate().filter(|(_,c)|!c.is_covered()).map(move |(column,c)|(row,column,c)))
                    .flat_map(move |(row,column,c)|c.text.paragraphs.iter().enumerate().map({let path=path.clone();move |(paragraph,p)|(json!({"slide":slide_index,"shape":path,"row":row,"column":column,"paragraph":paragraph,"kind":"table"}),Box::new(move ||p.text()) as Box<dyn FnOnce()->String>)}));
                text.chain(cells)
            });
            let notes=slide.notes.paragraphs.iter().enumerate().map(move |(paragraph,p)|(json!({"slide":slide_index,"paragraph":paragraph,"kind":"notes"}),Box::new(move ||p.text()) as Box<dyn FnOnce()->String>));
            shapes.chain(notes)
        });
        crate::extraction::page(units,unit,offset,limit,"Imported slide/group shapes, table cells and speaker notes in model order; no master/layout text, chart labels or OCR")
    }

    pub fn render(&mut self,part:u32,page:u32,width:u32)->Result<Raster,String> {
        if part!=0 || page as usize>=self.presentation.slides.len() {return Err("Invalid presentation slide".into());}
        let size=&self.presentation.slide_size;
        if !size.width.is_finite() || !size.height.is_finite() || size.width<=0.0 || size.height<=0.0 {return Err("Invalid slide geometry".into());}
        let scale=(f64::from(width)/size.width).min(4096.0/size.height);
        let before=deckcraft_render::panics_caught();
        let raster=deckcraft_render::render_slide(&self.presentation,page as usize,&RenderOpts {scale,threads:0,..Default::default()});
        if deckcraft_render::panics_caught()!=before {return Err("Craft slide rendering panicked".into());}
        Ok(Raster {width:raster.width,height:raster.height,format:0,pixels:raster.pixels})
    }
}

struct Shapes<'a> {stack:Vec<(&'a [Shape],usize,Vec<usize>)>}
impl<'a> Shapes<'a> {fn new(shapes:&'a [Shape])->Self{Self {stack:vec![(shapes,0,vec![])]}}}
impl<'a> Iterator for Shapes<'a> {
    type Item=(Vec<usize>,&'a Shape);
    fn next(&mut self)->Option<Self::Item> {
        while let Some((shapes,index,prefix))=self.stack.pop() {
            let Some(shape)=shapes.get(index) else {continue};
            self.stack.push((shapes,index+1,prefix.clone()));
            let mut path=prefix;path.push(index);
            if !shape.children().is_empty(){self.stack.push((shape.children(),0,path.clone()));}
            return Some((path,shape));
        }
        None
    }
}
