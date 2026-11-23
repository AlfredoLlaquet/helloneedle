use super::{paragraph_location, push_cell, Output, RichText, Style, MAX_BLOCKS, MAX_COLS, MAX_ROWS, MAX_TEXT_BYTES};
use super::package::parse;
use anyhow::{anyhow, bail, Result};
use roxmltree::Node;
use serde_json::json;
use std::collections::HashMap;

const OFFICE:&str="urn:oasis:names:tc:opendocument:xmlns:office:1.0";
const TEXT:&str="urn:oasis:names:tc:opendocument:xmlns:text:1.0";
const TABLE:&str="urn:oasis:names:tc:opendocument:xmlns:table:1.0";
const DRAW:&str="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
const STYLE:&str="urn:oasis:names:tc:opendocument:xmlns:style:1.0";
const FO:&str="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0";
const PRESENTATION:&str="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0";

type Styles=HashMap<(String,String),Formatting>;
struct Formatting {parent:String, bold:Option<bool>, italic:Option<bool>, underline:Option<bool>,strike:Option<bool>}
fn styles(root:Node<'_,'_>,out:&mut Styles){
    for n in root.descendants().filter(|n|n.has_tag_name((STYLE,"style"))){
        let (Some(name),Some(family))=(n.attribute((STYLE,"name")),n.attribute((STYLE,"family")))else{continue};
        if !matches!(family,"text"|"paragraph"){continue;}
        let props=n.children().find(|n|n.has_tag_name((STYLE,"text-properties")));
        let attr=|ns,local|props.and_then(|p|p.attribute((ns,local)));
        out.insert((family.into(),name.into()),Formatting{
            parent:n.attribute((STYLE,"parent-style-name")).unwrap_or("").into(),
            bold:attr(FO,"font-weight").map(|v|v=="bold"||v.parse::<u32>().is_ok_and(|n|n>=600)),
            italic:attr(FO,"font-style").map(|v|v=="italic"||v=="oblique"),
            underline:attr(STYLE,"text-underline-style").map(|v|v!="none"),
            strike:attr(STYLE,"text-line-through-style").map(|v|v!="none"),
        });
    }
}
fn styled(map:&Styles,mut base:Style,family:&str,name:&str)->Style{
    let mut chain=Vec::new();let mut key=(family.to_owned(),name.to_owned());
    while let Some(f)=map.get(&key){
        chain.push(f);
        if f.parent.is_empty()||chain.len()>=32 {break;}
        key.1.clone_from(&f.parent);
    }
    for f in chain.into_iter().rev(){
        if let Some(v)=f.bold{base.bold=v;}
        if let Some(v)=f.italic{base.italic=v;}
        if let Some(v)=f.underline{base.underline=v;}
        if let Some(v)=f.strike{base.strike=v;}
    }
    base
}

fn repeat(s:Option<&str>,limit:usize)->Result<usize>{
    let n=match s{Some(v)=>v.parse::<usize>().map_err(|_|anyhow!("corrupt_document: invalid repeat count"))?, None=>1};
    if n==0 {bail!("corrupt_document: zero repeat count");}
    if n>limit {bail!("resource_limit: repeat count");}
    Ok(n)
}
fn heading(n:Node<'_,'_>)->Option<usize>{
    n.has_tag_name((TEXT,"h")).then(||n.attribute((TEXT,"outline-level")).and_then(|s|s.parse().ok()).unwrap_or(1usize).clamp(1,10))
}
fn is_para(n:Node<'_,'_>)->bool{n.has_tag_name((TEXT,"p"))||n.has_tag_name((TEXT,"h"))}
fn ignore(n:Node<'_,'_>)->bool{
    n.has_tag_name((OFFICE,"annotation"))||n.has_tag_name((TEXT,"tracked-changes"))||
        n.has_tag_name((TEXT,"note"))||n.has_tag_name((PRESENTATION,"notes"))
}

fn paragraph(n:Node<'_,'_>,map:&Styles)->Result<RichText>{
    let mut rich=RichText::default();let mut pending=false;
    let name=n.attribute((TEXT,"style-name")).unwrap_or("");
    inline(n,map,styled(map,Style::default(),"paragraph",name),&mut rich,&mut pending,0)?;
    Ok(rich)
}
fn inline(n:Node<'_,'_>,map:&Styles,style:Style,rich:&mut RichText,pending:&mut bool,depth:usize)->Result<()>{
    if depth>128{bail!("resource_limit: OpenDocument text nesting");}
    for c in n.children(){
        if c.is_text(){
            for segment in c.text().unwrap_or("").split_inclusive([' ','\t','\n','\r']){
                let word=segment.trim_end_matches([' ','\t','\n','\r']);
                if !word.is_empty(){
                    if *pending && !rich.text.is_empty() {rich.append(" ",style)?;}
                    *pending=false;rich.append(word,style)?;
                }
                if word.len()<segment.len(){*pending=true;}
            }
        } else if c.is_element() && c.tag_name().namespace()==Some(TEXT){
            match c.tag_name().name(){
                "s"|"tab"|"line-break"=>{
                    if *pending && !rich.text.is_empty(){rich.append(" ",style)?;} *pending=false;
                    match c.tag_name().name(){
                        "s"=>{let count=repeat(c.attribute((TEXT,"c")),1_000_000)?; rich.append(&" ".repeat(count),style)?;}
                        "tab"=>rich.append("\t",style)?,
                        _=>rich.append("\n",style)?,
                    }
                }
                "note"|"ruby-text"|"tracked-changes"|"bookmark"|"bookmark-start"|"bookmark-end"=>{},
                _=>{
                    let st=c.attribute((TEXT,"style-name")).map_or(style,|name|styled(map,style,"text",name));
                    inline(c,map,st,rich,pending,depth+1)?;
                }
            }
        }
    }
    Ok(())
}

pub(super) fn extract(xml:&str,external_styles:Option<&str>,kind:&str,out:&mut Output)->Result<()>{
    let doc=parse(xml)?;let root=doc.root_element();
    if !root.has_tag_name((OFFICE,"document-content"))&&!root.has_tag_name((OFFICE,"document")){
        bail!("corrupt_document: invalid OpenDocument root");
    }
    let expected=match kind{"ods"=>"spreadsheet","odp"=>"presentation",_=>"text"};
    let body=root.children().find(|n|n.has_tag_name((OFFICE,"body")))
        .and_then(|n|n.children().find(|n|n.has_tag_name((OFFICE,expected))))
        .ok_or_else(||anyhow!("corrupt_document: missing OpenDocument {expected} body"))?;
    let mut map=Styles::new();
    if let Some(xml)=external_styles{styles(parse(xml)?.root_element(),&mut map);}
    styles(root,&mut map);
    match kind{
        "ods"=>spreadsheet(body,&map,out),
        "odp"=>presentation(body,&map,out),
        _=>writer(body,&map,None,&mut (0,0),out,0),
    }
}

fn writer(n:Node<'_,'_>,map:&Styles,cell:Option<(usize,usize,usize)>,counts:&mut(usize,usize),out:&mut Output,depth:usize)->Result<()>{
    if depth>128{bail!("resource_limit: OpenDocument nesting");}
    for c in n.children().filter(|n|n.is_element()){
        if is_para(c){
            counts.0+=1;
            let h=heading(c);let (text,html)=paragraph(c,map)?.finish(h);
            let (loc,label)=paragraph_location(counts.0,h,cell);out.push(text,loc,label,Some(html))?;
        }else if c.has_tag_name((TABLE,"table")){
            counts.1+=1;let table=counts.1;
            for (idx,row) in table_rows(c).enumerate(){
                let mut col=1;
                for tc in row.children().filter(|n|is_cell(*n)){
                    if tc.has_tag_name((TABLE,"table-cell")){writer(tc,map,Some((table,idx+1,col)),counts,out,depth+1)?;}
                    col+=repeat(tc.attribute((TABLE,"number-columns-repeated")),MAX_COLS)?;
                }
            }
        }else if !ignore(c){writer(c,map,cell,counts,out,depth+1)?;}
    }
    Ok(())
}

fn is_cell(n:Node<'_,'_>)->bool{n.has_tag_name((TABLE,"table-cell"))||n.has_tag_name((TABLE,"covered-table-cell"))}
fn table_rows<'a,'i>(table:Node<'a,'i>)->impl Iterator<Item=Node<'a,'i>>{
    table.descendants().filter(move |n|n.has_tag_name((TABLE,"table-row"))&&
        n.ancestors().find(|a|a.has_tag_name((TABLE,"table")))==Some(table))
}
fn cell_text(n:Node<'_,'_>,map:&Styles)->Result<String>{
    let mut text=String::new();
    for p in n.children().filter(|c|is_para(*c)){
        if !text.is_empty(){text.push('\n');}
        text.push_str(&paragraph(p,map)?.text);
        if text.len()>MAX_TEXT_BYTES{bail!("resource_limit: spreadsheet cell");}
    }
    let attr=match n.attribute((OFFICE,"value-type")){
        Some("date")=>"date-value",Some("time")=>"time-value",Some("boolean")=>"boolean-value",
        Some("string")=>"string-value",_=>"value",
    };
    if let Some(value)=n.attribute((OFFICE,attr)).filter(|s|!s.is_empty()){
        if text.is_empty(){text.push_str(value);}
        else if value!=text && attr!="string-value"{text.push('\n');text.push_str(value);}
    }
    Ok(text)
}
fn spreadsheet(body:Node<'_,'_>,map:&Styles,out:&mut Output)->Result<()>{
    for table in body.children().filter(|n|n.has_tag_name((TABLE,"table"))){
        let sheet=table.attribute((TABLE,"name")).ok_or_else(||anyhow!("corrupt_document: unnamed sheet"))?;
        let mut rownum=1;
        for row in table_rows(table){
            let rows=repeat(row.attribute((TABLE,"number-rows-repeated")),MAX_ROWS)?;
            if rownum+rows-1>MAX_ROWS{bail!("resource_limit: spreadsheet rows");}
            let mut col=1;let mut cells=Vec::new();let mut block_count=0usize;let mut estimated_bytes=0usize;
            for c in row.children().filter(|n|is_cell(*n)){
                let cols=repeat(c.attribute((TABLE,"number-columns-repeated")),MAX_COLS)?;
                if col+cols-1>MAX_COLS{bail!("resource_limit: spreadsheet columns");}
                if c.has_tag_name((TABLE,"table-cell")){
                    let value=cell_text(c,map)?;
                    if !value.trim().is_empty(){
                        block_count+=cols;
                        estimated_bytes=estimated_bytes.saturating_add(value.len().saturating_mul(cols));
                        cells.push((col,cols,value));
                    }
                }
                col+=cols;
            }
            if block_count.saturating_mul(rows)>MAX_BLOCKS-out.blocks.len() ||
                estimated_bytes.saturating_mul(rows)>MAX_TEXT_BYTES-out.bytes{
                bail!("resource_limit: repeated spreadsheet data");
            }
            if !cells.is_empty(){
                for r in rownum..rownum+rows{for (c,cols,value) in &cells{for cc in *c..c+cols {
                    push_cell(out,sheet,r,cc,value.clone())?;
                }}}
            }
            rownum+=rows;
        }
    }
    Ok(())
}

fn presentation(body:Node<'_,'_>,map:&Styles,out:&mut Output)->Result<()>{
    for (idx,page) in body.children().filter(|n|n.has_tag_name((DRAW,"page"))).enumerate(){
        let slide=idx+1;
        for (idx,p) in page.descendants().filter(|&n|is_para(n)&&!n.ancestors().any(ignore)).enumerate(){
            let paragraph_number=idx+1;let (text,html)=paragraph(p,map)?.finish(heading(p));
            out.push(text,json!({"slide":slide,"paragraph":paragraph_number}),format!("Slide {slide} · paragraph {paragraph_number}"),Some(html))?;
        }
    }
    Ok(())
}
