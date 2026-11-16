use crate::{model::Result, win};
use serde_json::Value;
use std::{collections::BTreeSet, fs::File, io::Write, path::Path};
use zip::{ZipWriter, write::SimpleFileOptions};

fn text(value: &Value) -> String {
    match value { Value::Null=>String::new(), Value::String(s)=>s.clone(), _=>value.to_string() }
}
fn escape(value: &str) -> String {
    value.chars().filter(|c|matches!(*c,'\t'|'\r'|'\n')||*c>=' '&&!matches!(*c,'\u{fffe}'|'\u{ffff}')).collect::<String>().replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;").replace('"',"&quot;")
}
fn csv_cell(value: &str) -> String {
    let value = if value.trim_start().starts_with(['=','+','-','@'])||value.starts_with(['\t','\r']) { format!("'{value}") } else {value.into()};
    format!("\"{}\"",value.replace('"',"\"\""))
}
pub fn inventory_format(path:&Path,results:&[Value],selected:&[String],format:&str)->Result<()>{
    match format.to_lowercase().as_str() {
        "xlsx" => workbook(path, results, selected),
        "csv" => {
            let mut file=win::io(File::create(path))?;
            win::io(file.write_all(b"\xef\xbb\xbf"))?;
            win::io(file.write_all(b"computer,category,field,value\r\n"))?;
            for result in results {
                let computer=text(&result["computer"]);
                if result["success"] != true {
                    let row=[computer.clone(),"error".into(),"error".into(),text(&result["error"])];
                    win::io(writeln!(file,"{}\r",row.iter().map(|s|csv_cell(s)).collect::<Vec<_>>().join(",")))?;
                }
                for category in selected {
                    for item in result["data"][category].as_array().cloned().unwrap_or_else(||vec![result["data"][category].clone()]) {
                        let item=if item.is_null(){continue;}else if item.is_object(){item}else{serde_json::json!({"value":item})};
                        if let Some(object)=item.as_object() {
                            for (field,value) in object {
                                let row=[computer.clone(),category.clone(),field.clone(),text(value)];
                                win::io(writeln!(file,"{}\r",row.iter().map(|s|csv_cell(s)).collect::<Vec<_>>().join(",")))?;
                            }
                        }
                    }
                }
            }
            Ok(())
        },
        _ => {
            let filtered:Vec<_>=results.iter().map(|result| {
                let mut result=result.clone();
                if let Some(data)=result["data"].as_object_mut() {data.retain(|key,_|selected.contains(key)||["computer","collectedAt"].contains(&key.as_str()));}
                result
            }).collect();
            win::io(std::fs::write(path,serde_json::to_vec_pretty(&filtered).map_err(|e|e.to_string())?))
        }
    }
}
fn workbook(path:&Path,results:&[Value],categories:&[String])->Result<()> {
    let mut writer=ZipWriter::new(win::io(File::create(path))?);
    let options=SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut sheets=vec![("结果".to_owned(),vec![
        vec!["电脑".into(),"状态".into(),"错误".into()]
    ])];
    for result in results {
        sheets[0].1.push(vec![text(&result["computer"]),if result["success"]==true {"成功".into()} else {"失败".into()},text(&result["error"])]);
    }
    for category in categories {
        let mut columns=BTreeSet::new();
        let mut items=Vec::new();
        for result in results {
            for item in result["data"][category].as_array().cloned().unwrap_or_else(||vec![result["data"][category].clone()]) {
                let item=if item.is_null(){continue;}else if item.is_object(){item}else{serde_json::json!({"value":item})};
                if let Some(object)=item.as_object() {
                    columns.extend(object.keys().cloned());
                    items.push((text(&result["computer"]),object.clone()));
                }
            }
        }
        let columns:Vec<_>=columns.into_iter().collect();
        let mut rows=vec![std::iter::once("computer".into()).chain(columns.iter().cloned()).collect()];
        for (computer,item) in items {
            rows.push(std::iter::once(computer).chain(columns.iter().map(|name|text(item.get(name).unwrap_or(&Value::Null)))).collect());
        }
        sheets.push((category.chars().take(31).collect(),rows));
    }
    let mut types=String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>");
    let mut workbook=String::from("<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>");
    let mut rels=String::from("<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
    for (index,(name,rows)) in sheets.iter().enumerate() {
        let n=index+1;
        types.push_str(&format!("<Override PartName=\"/xl/worksheets/sheet{n}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"));
        workbook.push_str(&format!("<sheet name=\"{}\" sheetId=\"{n}\" r:id=\"rId{n}\"/>",escape(name)));
        rels.push_str(&format!("<Relationship Id=\"rId{n}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{n}.xml\"/>"));
        let mut xml=String::from("<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>");
        for row in rows {
            xml.push_str("<row>");
            for cell in row {xml.push_str(&format!("<c t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",escape(&cell.chars().filter(|c|!c.is_control()||['\t','\r','\n'].contains(c)).take(32767).collect::<String>())));}
            xml.push_str("</row>");
        }
        xml.push_str("</sheetData></worksheet>");
        writer.start_file(format!("xl/worksheets/sheet{n}.xml"),options).map_err(|e|e.to_string())?;
        win::io(writer.write_all(xml.as_bytes()))?;
    }
    types.push_str("</Types>");workbook.push_str("</sheets></workbook>");rels.push_str("</Relationships>");
    for (name,xml) in [
        ("[Content_Types].xml",types),
        ("xl/workbook.xml",workbook),
        ("xl/_rels/workbook.xml.rels",rels),
        ("_rels/.rels","<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>".into())
    ] {
        writer.start_file(name,options).map_err(|e|e.to_string())?;
        win::io(writer.write_all(xml.as_bytes()))?;
    }
    writer.finish().map_err(|e|e.to_string())?;
    Ok(())
}
#[cfg(test)]mod tests{
    use super::*;
    struct Output(std::path::PathBuf);impl Drop for Output{fn drop(&mut self){let _=std::fs::remove_file(&self.0);}}
    fn output(extension:&str)->Output{Output(std::env::temp_dir().join(format!("CorpRustExportTest-{}.{extension}",crate::model::new_id())))}
    #[test]fn warnings_survive_csv_and_formula_text_is_preserved(){
        let path=output("csv");let records=[serde_json::json!({"computer":"fixture","success":true,"data":{"warnings":["  =1+1","comma, quote\""]}})];
        inventory_format(&path.0,&records,&["warnings".into()],"csv").unwrap();let text=std::fs::read_to_string(&path.0).unwrap();assert!(text.contains("'  =1+1"));assert!(text.contains("comma, quote\"\""));
    }
    #[test]fn workbook_is_valid_xml_and_includes_scalar_warnings(){
        use std::io::Read;
        let path=output("user-extension");let records=[serde_json::json!({"computer":"fixture","success":true,"data":{"warnings":["warning & <value>\u{1}"]}})];
        inventory_format(&path.0,&records,&["warnings".into()],"xlsx").unwrap();let mut archive=zip::ZipArchive::new(File::open(&path.0).unwrap()).unwrap();let mut warning_found=false;
        for index in 0..archive.len(){let mut item=archive.by_index(index).unwrap();let mut text=String::new();item.read_to_string(&mut text).unwrap();let document=roxmltree::Document::parse(&text).unwrap();
            if item.name()=="xl/worksheets/sheet2.xml"{warning_found=document.descendants().filter_map(|node|node.text()).any(|value|value=="warning & <value>");}
        }assert!(warning_found);
    }
}
