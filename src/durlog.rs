//! Append-only durable line log: each append is written and fsynced before
//! it returns. Replay yields complete lines only; a trailing fragment left
//! by a crash mid-append is dropped.

use std::fs::File;
use std::io;
use std::path::Path;

pub struct DurLog {
    file: File,
}

impl DurLog {
    pub fn open(path: &Path) -> io::Result<(DurLog, Vec<String>)> {
        use std::io::Read;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let existed = path.exists();
        let mut bytes = Vec::new();
        if existed {
            File::open(path)?.read_to_end(&mut bytes)?;
        }
        // A crash mid-append leaves a fragment with no terminating newline.
        // Drop it from the file, not just from the replay: an append in
        // append mode would otherwise glue onto it and corrupt a record.
        let keep = match bytes.iter().rposition(|&b| b == b'\n') {
            Some(i) => i + 1,
            None => 0,
        };
        if keep < bytes.len() {
            std::fs::OpenOptions::new()
                .write(true)
                .open(path)?
                .set_len(keep as u64)?;
            bytes.truncate(keep);
        }
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<String> = text
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect();
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        if !existed {
            // Make the new file itself durable.
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    File::open(parent)?.sync_all()?;
                }
            }
        }
        Ok((DurLog { file }, lines))
    }

    pub fn append(&mut self, line: &str) -> io::Result<()> {
        use std::io::Write;
        debug_assert!(!line.contains('\n'));
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.file.sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("roger-durlog-{}-{tag}.log", std::process::id()));
        let _ = fs::remove_file(&p);
        p
    }

    #[test]
    fn appended_lines_survive_reopen_in_order() {
        let path = temp_path("order");
        {
            let (mut log, prior) = DurLog::open(&path).unwrap();
            assert!(prior.is_empty());
            log.append("{\"a\":1}").unwrap();
            log.append("{\"b\":2}").unwrap();
        }
        let (_log, lines) = DurLog::open(&path).unwrap();
        assert_eq!(
            lines,
            vec!["{\"a\":1}".to_string(), "{\"b\":2}".to_string()]
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn replay_drops_trailing_partial_line() {
        let path = temp_path("partial");
        {
            let mut f = File::create(&path).unwrap();
            f.write_all(b"{\"a\":1}\n{\"b\":2").unwrap(); // crash mid-append
        }
        let (_log, lines) = DurLog::open(&path).unwrap();
        assert_eq!(lines, vec!["{\"a\":1}".to_string()]);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn append_after_a_crash_fragment_does_not_glue_onto_it() {
        let path = temp_path("glue");
        {
            let mut f = File::create(&path).unwrap();
            f.write_all(b"{\"a\":1}\n{\"b\":2").unwrap(); // crash mid-append
        }
        {
            let (mut log, _) = DurLog::open(&path).unwrap();
            log.append("{\"c\":3}").unwrap();
        }
        let (_log, lines) = DurLog::open(&path).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert_eq!(
            lines,
            vec!["{\"a\":1}".to_string(), "{\"c\":3}".to_string()],
            "raw file: {raw:?}"
        );
    }
}
