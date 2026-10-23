//! Append-only durable line log: each append is written and fsynced before
//! it returns. Replay yields complete lines only; a trailing fragment left
//! by a crash mid-append is dropped.
//!
//! The file is locked exclusively for as long as the log is open, so a
//! second process (a second adapter on one Store, a second daemon or
//! coordinator on one data file) fails at open instead of numbering records
//! from the same replay. The kernel drops the lock when the process dies.
//!
//! One failed append poisons the log: the bytes may have reached the disk
//! even though the caller saw an error and left memory unchanged, so no
//! later append may commit on top of that split. Every later append returns
//! an error naming the first failure; the owner restarts and replays.

use std::fs::File;
use std::io;
use std::path::Path;

pub struct DurLog {
    file: File,
    /// The first append failure, kept so every later append refuses.
    poison: Option<String>,
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
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        // no-poll: one attempt, never retried; a held lock is an error.
        file.try_lock().map_err(|e| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                format!(
                    "expected {} unlocked, saw it held by another process: {e}",
                    path.display()
                ),
            )
        })?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        // A crash mid-append leaves a fragment with no terminating newline.
        // Drop it from the file, not just from the replay: an append in
        // append mode would otherwise glue onto it and corrupt a record.
        let keep = match bytes.iter().rposition(|&b| b == b'\n') {
            Some(i) => i + 1,
            None => 0,
        };
        if keep < bytes.len() {
            file.set_len(keep as u64)?;
            bytes.truncate(keep);
        }
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<String> = text
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect();
        if !existed {
            // Make the new file itself durable.
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    File::open(parent)?.sync_all()?;
                }
            }
        }
        Ok((DurLog { file, poison: None }, lines))
    }

    #[cfg(test)]
    fn from_file(file: File) -> DurLog {
        DurLog { file, poison: None }
    }

    /// True once an append has failed: nothing appended since has committed.
    pub fn is_poisoned(&self) -> bool {
        self.poison.is_some()
    }

    pub fn append(&mut self, line: &str) -> io::Result<()> {
        debug_assert!(!line.contains('\n'));
        if let Some(first) = &self.poison {
            return Err(io::Error::other(format!(
                "expected a durable log, saw one poisoned by an earlier append failure: {first}"
            )));
        }
        self.write_line(line)
            .inspect_err(|e| self.poison = Some(e.to_string()))
    }

    fn write_line(&mut self, line: &str) -> io::Result<()> {
        use std::io::Write;
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
    fn one_failed_append_poisons_every_later_append() {
        // /dev/full accepts the open and fails every write with ENOSPC, so
        // the first append fails for a real filesystem reason and the second
        // must fail for the poison alone.
        let file = fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let mut log = DurLog::from_file(file);
        assert!(!log.is_poisoned());
        let first = log.append("{\"a\":1}").unwrap_err();
        assert!(log.is_poisoned());
        let second = log.append("{\"b\":2}").unwrap_err();
        let msg = second.to_string();
        assert!(msg.contains("poisoned"), "{msg}");
        assert!(msg.contains(&first.to_string()), "{msg} / {first}");
    }

    #[test]
    fn a_second_open_of_a_held_log_fails_until_the_first_closes() {
        let path = temp_path("lock");
        let (first, _) = DurLog::open(&path).unwrap();
        let err = DurLog::open(&path).err().expect("second open must fail");
        assert!(err.to_string().contains("held by another process"), "{err}");
        drop(first);
        DurLog::open(&path).unwrap();
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
