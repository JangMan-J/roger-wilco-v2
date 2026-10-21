//! A contract-shaped agent session: registers with its host daemon,
//! writes delivered mail to its own durable Store, and acks naming the
//! Store record. It knows nothing about the core beyond the socket.

use crate::durlog::DurLog;
use crate::json::{self, obj, s, Value};
use crate::net;
use std::collections::HashMap;
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

pub fn run(
    socket: PathBuf,
    id: String,
    store_path: PathBuf,
    hold_before_ack: Option<PathBuf>,
) -> io::Result<()> {
    let (mut store, prior) = DurLog::open(&store_path)?;
    let mut records: HashMap<String, String> = HashMap::new(); // mail -> record
    for line in &prior {
        if let Ok(v) = json::parse(line) {
            if let (Ok(mail), Ok(record)) = (net::field(&v, "mail"), net::field(&v, "record")) {
                records.insert(mail.to_string(), record.to_string());
            }
        }
    }
    let mut next_record = records.len() as u64 + 1;

    let conn = UnixStream::connect(&socket)?;
    let writer = net::writer(Box::new(conn.try_clone()?));
    let lines = net::lines(Box::new(conn));
    net::send(
        &writer,
        &obj(vec![("type", s("register")), ("session", s(&id))]),
    )?;
    println!("READY session={id}");
    io::stdout().flush()?;

    let mut req = 0i64;
    for line in lines {
        let v = line?;
        match net::field(&v, "type")? {
            "deliver" => {
                let mail = net::field(&v, "mail")?.to_string();
                let record = match records.get(&mail) {
                    Some(existing) => existing.clone(),
                    None => {
                        let record = format!("r-{next_record}");
                        next_record += 1;
                        let entry = obj(vec![
                            ("record", s(&record)),
                            ("mail", s(&mail)),
                            ("from", s(net::field(&v, "from")?)),
                            ("body", s(net::field(&v, "body")?)),
                        ]);
                        store.append(&json::stringify(&entry))?;
                        records.insert(mail.clone(), record.clone());
                        println!("STORED mail={mail} record={record}");
                        io::stdout().flush()?;
                        record
                    }
                };
                if let Some(marker) = &hold_before_ack {
                    if !marker.exists() {
                        std::fs::write(marker, "held\n")?;
                        // Test hook: sit between store write and ack until
                        // killed. On restart the marker exists and we ack.
                        loop {
                            std::thread::sleep(std::time::Duration::from_secs(3600));
                        }
                    }
                }
                req += 1;
                net::send(
                    &writer,
                    &obj(vec![
                        ("type", s("ack")),
                        ("req", Value::Int(req)),
                        ("mail", s(&mail)),
                        ("record", s(&record)),
                    ]),
                )?;
            }
            "acked" => {
                println!(
                    "DELIVERED mail={} record={}",
                    net::field(&v, "mail")?,
                    net::field(&v, "record")?
                );
                io::stdout().flush()?;
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn send(
    socket: PathBuf,
    from: String,
    to: String,
    mail: String,
    body: String,
) -> io::Result<()> {
    let conn = UnixStream::connect(&socket)?;
    let writer = net::writer(Box::new(conn.try_clone()?));
    let mut lines = net::lines(Box::new(conn));
    net::send(
        &writer,
        &obj(vec![
            ("type", s("send")),
            ("req", Value::Int(1)),
            ("mail", s(&mail)),
            ("from", s(&from)),
            ("to", s(&to)),
            ("body", s(&body)),
        ]),
    )?;
    for line in &mut lines {
        let v = line?;
        if net::field(&v, "type")? == "accepted" {
            println!("ACCEPTED mail={}", net::field(&v, "mail")?);
            return Ok(());
        }
    }
    Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no accepted"))
}

pub fn gen_uuid() -> io::Result<String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13],
        b[14], b[15]
    ))
}

use std::io::Read as _;
