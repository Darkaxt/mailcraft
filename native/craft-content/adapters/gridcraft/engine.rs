// SPDX-FileCopyrightText: 2026 Protagonist Labs, Pathweft and MailCraft contributors
// SPDX-License-Identifier: MIT
use crate::Raster;
use serde_json::{json,Value};
use gridcraft_engine::model::Workbook;
use gridcraft_engine::cmd::print::{SheetLayout,PrintOptions,layout_sheet,render_pdf};

// GridCraft's existing non-UI worksheet painter is its print/PDF core. Preserve
// that painter rather than implementing a second cell renderer. Its in-memory
// display list is rasterised inside the same worker using Windows' existing PDF
// API. No CLI, intermediate file or separate PDF viewer process participates.
pub struct Engine { workbook:Workbook, layouts:Vec<SheetLayout>, notes:Vec<String> }

impl Engine {
    pub fn open(bytes:&[u8],extension:&str)->Result<Self,String> {
        let (workbook,report)=match extension {
            "xlsx"|"xlsm"=>gridcraft_xlsx::read_xlsx(bytes),
            "xls"=>gridcraft_xlsx::read_xls(bytes),
            "xlsb"=>gridcraft_xlsx::read_xlsb(bytes),
            "ods"=>gridcraft_xlsx::read_ods(bytes),
            _=>return Err("Unsupported spreadsheet format".into()),
        }.map_err(|e|e.to_string())?;
        if workbook.sheets.is_empty() || workbook.sheets.len()>1000 {return Err("Invalid workbook sheet count".into());}
        let layouts=(0..workbook.sheets.len()).map(|i|layout_sheet(&workbook,i,&PrintOptions::default(),None).ok_or_else(||"Could not lay out worksheet".to_string())).collect::<Result<Vec<_>,_>>()?;
        Ok(Self {workbook,layouts,notes:report.warnings})
    }

    pub fn info(&self)->Value {
        let parts:Vec<_>=self.layouts.iter().map(|layout|json!({"name":self.workbook.sheets[layout.sheet].name,"pages":layout.pages.len()})).collect();
        json!({"engine":"GridCraft","kind":"spreadsheet","parts":parts,"notes":self.notes})
    }

    pub fn extract(&self,unit:u64,offset:usize,limit:usize)->Result<Value,String> {
        let units=self.workbook.sheets.iter().enumerate().flat_map(|(sheet_index,sheet)|{
            let name=std::iter::once((json!({"sheet":sheet_index,"kind":"sheetName"}),Box::new(move ||sheet.name.clone()) as Box<dyn FnOnce()->String + '_>));
            let cells=sheet.cells.iter().filter(|(_,cell)|cell.has_content()).flat_map(move |(address,cell)| {
                let location=|kind|json!({"sheet":sheet_index,"sheetName":sheet.name,"row":address.row,"column":address.col,"cell":address.a1(),"kind":kind});
                let mut units:Vec<(Value,Box<dyn FnOnce()->String + '_>)>=Vec::with_capacity(3);
                units.push((location("cachedValue"),Box::new(move ||cell.value.display())));
                if let Some(formula)=&cell.formula {
                    units.push((location("formula"),Box::new(move ||formula.text.clone())));
                }
                if let Some(python)=&cell.py {
                    units.push((location("pythonSource"),Box::new(move ||python.code.clone())));
                }
                units
            });
            name.chain(cells)
        });
        crate::extraction::page(units,unit,offset,limit,"Imported sheet names, stored cell values, formula source and Python source in sheet/row/column order; code and formulas are never executed; rendered-page OCR is separately attributed")
    }

    pub fn render(&mut self,part:u32,page:u32,_width:u32)->Result<Raster,String> {
        let layout=self.layouts.get(part as usize).ok_or("Invalid worksheet")?;
        if page as usize>=layout.pages.len() {return Err("Invalid worksheet page".into());}
        let (pixels,count)=render_pdf(&self.workbook,std::slice::from_ref(layout),"","");
        if count!=layout.pages.len() {return Err("Worksheet painter returned a different page count".into());}
        Ok(Raster {width:0,height:0,format:1,pixels})
    }
}
