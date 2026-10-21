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
        let text = String::from_utf8_lossy(&bytes);
        let complete = if text.ends_with('\n') || text.is_empty() {
            text.as_ref()
        } else {
            match text.rfind('\n') {
                Some(i) => &text.as_ref()[..=i],
                None => "",
            }
        };
        let lines: Vec<String> = complete
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
}
