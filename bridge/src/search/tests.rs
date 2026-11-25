// SPDX-License-Identifier: GPL-3.0-or-later
//! Exercise per-file rollback while other readers continue to emit occurrences.
use crate::protocol;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

struct ChangeDuringSearch {
    changed: Option<PathBuf>,
    line: Vec<u8>,
    retained: HashMap<PathBuf, Vec<(String, usize)>>,
    diagnostics: Vec<String>,
    done: Value,
}

impl Write for ChangeDuringSearch {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.line.extend_from_slice(bytes);
        if self.line.last() != Some(&b'\n') {
            return Ok(bytes.len());
        }
        let event: Value = serde_json::from_slice(&self.line)?;
        self.line.clear();
        let path = Path::new(event["path"].as_str().unwrap_or(""));
        match event["type"].as_str().unwrap() {
            "match" => {
                let kind = event["kind"].as_str().unwrap();
                if kind != "filename" && self.changed.is_none() {
                    // The protocol has received a provisional content hit. The file
                    // is longer than the bounded event queue, so its final revision
                    // check is still outstanding when we append this line.
                    OpenOptions::new()
                        .append(true)
                        .open(path)?
                        .write_all(b"changed\n")?;
                    self.changed = Some(path.to_owned());
                }
                self.retained.entry(path.to_owned()).or_default().push((
                    kind.to_owned(),
                    event["location"]["index"].as_u64().unwrap() as usize,
                ));
            }
            "error" => self
                .diagnostics
                .push(event["code"].as_str().unwrap().into()),
            "discard" => self
                .retained
                .get_mut(path)
                .unwrap()
                .retain(|(kind, _)| kind == "filename"),
            "done" => self.done = event,
            _ => {}
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn changed_content_is_withdrawn_without_losing_names_or_other_files() {
    for stable_lines in [7_013, 10_009] {
        let dir = std::env::temp_dir().join(format!(
            "hn-file-revision-{}-{stable_lines}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        let files = [dir.join("a-mark.txt"), dir.join("b-mark.txt")];
        for path in &files {
            fs::write(path, "mark\n".repeat(stable_lines)).unwrap();
        }
        let mut output = ChangeDuringSearch {
            changed: None,
            line: Vec::new(),
            retained: HashMap::new(),
            diagnostics: Vec::new(),
            done: Value::Null,
        };
        let request = json!({"folder":dir, "query":"mark", "searchWorkers":4, "filenames":true});
        protocol::run("search", request.to_string().as_bytes(), &mut output).unwrap();
        fs::remove_dir_all(dir).unwrap();
        let changed = output.changed.as_ref().unwrap();
        let stable = files.iter().find(|path| *path != changed).unwrap();
        assert_eq!(output.diagnostics, ["changed"]);
        assert_eq!(output.retained[changed], [("filename".into(), 0)]);
        let stable_hits = &output.retained[stable];
        assert_eq!(stable_hits[0].0, "filename");
        let total = stable_lines + 2;
        assert_eq!(stable_hits.len() + 1, total);
        for (index, (kind, actual)) in stable_hits[1..].iter().enumerate() {
            assert_eq!(kind, "text");
            assert_eq!(
                index, *actual,
                "An occurrence from the stable file was duplicated or missed"
            );
        }
        assert_eq!(output.done["count"], total);
        assert_eq!(output.done["files"], 2);
        assert_eq!(output.done["documentsScanned"], 1);
        assert_eq!(output.done["truncated"], false);
    }
}
