use super::{Output, MAX_TEXT_BYTES};
use anyhow::{anyhow, bail, Result};
use serde_json::json;
use std::{
    env, fs::File, io::Read, path::{Path, PathBuf}, process::{Command, Stdio},
    thread, time::{Duration,Instant},
};

fn executable()->Option<PathBuf>{
    let name=if cfg!(windows){"antiword.exe"}else{"antiword"};
    let bundled=env::current_exe().ok().and_then(|p|p.parent().map(|d|d.join(name)));
    bundled.filter(|p|p.is_file()).or_else(||
        env::split_paths(&env::var_os("PATH").unwrap_or_default())
            .filter(|dir|dir.is_absolute()).map(|dir|dir.join(name)).find(|p|p.is_file()))
}

pub(super) fn extract(path:&Path,mut file:File,out:&mut Output)->Result<()>{
    let mut sig=[0u8;8];let n=file.read(&mut sig)?;
    if n<2 {bail!("corrupt_document: truncated Word file");}
    if &sig!=b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1" && !matches!(&sig[..2],b"\xdb\xa5"|b"\xdc\xa5"|b"\x31\xbe"|b"\x32\xbe"){
        bail!("unsupported_document: expected a legacy binary Word file");
    }
    let program=executable().ok_or_else(||anyhow!("missing_decoder: antiword"))?;
    let mut child=Command::new(program).args(["-t","-w","0","-m","UTF-8.txt"])
        .arg(path.canonicalize()?).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e|anyhow!("missing_decoder: antiword: {e}"))?;
    let stdout=child.stdout.take().unwrap();let stderr=child.stderr.take().unwrap();
    let output=thread::spawn(move ||{let mut b=Vec::new();stdout.take(MAX_TEXT_BYTES as u64+1).read_to_end(&mut b).map(|_|b)});
    let diagnostics=thread::spawn(move ||{let mut b=Vec::new();let _=stderr.take(8192).read_to_end(&mut b);b});
    let started=Instant::now();
    let status=loop {
        if let Some(s)=child.try_wait()?{break s;}
        if started.elapsed()>Duration::from_secs(20){
            let _=child.kill();let _=child.wait();
            bail!("resource_limit: Word decoder timeout");
        }
        thread::sleep(Duration::from_millis(15));
    };
    let b=output.join().map_err(|_|anyhow!("corrupt_document: Word decoder output"))??;
    let diag=String::from_utf8_lossy(&diagnostics.join().unwrap_or_default()).to_lowercase();
    if b.len()>MAX_TEXT_BYTES {bail!("resource_limit: Word decoder output");}
    if !status.success(){
        if diag.contains("encrypted")||diag.contains("password"){bail!("encrypted_document: binary Word");}
        if diag.contains("mapping")||diag.contains("utf-8.txt"){bail!("missing_decoder: antiword UTF-8 map");}
        if diag.contains("not supported")||diag.contains("not a word document")||diag.contains("too small to handle"){bail!("unsupported_document: legacy Word variant");}
        bail!("corrupt_document: antiword could not read this file");
    }
    let text=std::str::from_utf8(&b).map_err(|_|anyhow!("corrupt_document: invalid Word decoder text"))?;
    let mut number=0;
    for line in text.lines(){
        if !line.trim().is_empty(){
            number+=1;out.push(line.to_owned(),json!({"paragraph":number}),format!("Paragraph {number}"),None)?;
        }
    }
    Ok(())
}
