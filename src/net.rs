//! Line-delimited JSON over any byte stream. One object per line.

use crate::json::{self, obj, s, Value};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

pub type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

/// The one shape a host daemon uses to forward a receipt upstream, both on
/// the ack itself and when replaying receipts after a reconnect.
pub fn ack_msg(mail: &str, session: &str, record: &str) -> Value {
    obj(vec![
        ("type", s("ack")),
        ("mail", s(mail)),
        ("session", s(session)),
        ("record", s(record)),
    ])
}

/// The one shape both the coordinator and a host daemon use to hand a Mail
/// to the next hop.
pub fn deliver_msg(mail: &str, from: &str, to: &str, body: &str) -> Value {
    obj(vec![
        ("type", s("deliver")),
        ("mail", s(mail)),
        ("from", s(from)),
        ("to", s(to)),
        ("body", s(body)),
    ])
}

pub fn writer(w: Box<dyn Write + Send>) -> Writer {
    Arc::new(Mutex::new(w))
}

pub fn send(w: &Writer, v: &Value) -> io::Result<()> {
    let mut guard = w.lock().unwrap();
    guard.write_all(json::stringify(v).as_bytes())?;
    guard.write_all(b"\n")?;
    guard.flush()
}

pub struct Lines {
    inner: std::io::Lines<BufReader<Box<dyn Read + Send>>>,
}

pub fn lines(r: Box<dyn Read + Send>) -> Lines {
    Lines {
        inner: BufReader::new(r).lines(),
    }
}

impl Iterator for Lines {
    type Item = io::Result<Value>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next()? {
                Ok(line) if line.trim().is_empty() => continue,
                Ok(line) => {
                    return Some(
                        json::parse(&line)
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
                    )
                }
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

pub fn field<'a>(v: &'a Value, key: &str) -> io::Result<&'a str> {
    v.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("missing {key}")))
}
