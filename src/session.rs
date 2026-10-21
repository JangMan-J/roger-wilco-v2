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

/// The Tracer: a session with no harness, reporting on its own stdout.
pub fn run(
    socket: PathBuf,
    id: String,
    store_path: PathBuf,
    hold_before_ack: Option<PathBuf>,
) -> io::Result<()> {
    serve(socket, id, store_path, hold_before_ack, None)
}

/// The adapter: the same session, fronted by one line-delimited JSON stream
/// on stdin/stdout for a shim. The shim sends `{"type":"send","to","body"}`
/// (optional `"mail"`) and is told `ready`, `accepted`, `error`, and `mail`.
/// A `mail` line follows the Store write and never gates the ack: the shim
/// renders what the adapter already holds and acknowledges nothing.
pub fn adapter(
    socket: PathBuf,
    id: String,
    store_path: PathBuf,
    hold_before_ack: Option<PathBuf>,
) -> io::Result<()> {
    let out = net::writer(Box::new(io::stdout()));
    let (sock, me, to_shim) = (socket.clone(), id.clone(), out.clone());
    // A send blocks until the core answers, so sends run on a worker, one at a
    // time and in order (a shim pairs replies to sends by order). The reader
    // below only reads stdin, so the shim's EOF ends this process at once even
    // while a send waits.
    let (tx, rx) = std::sync::mpsc::channel::<io::Result<Value>>();
    std::thread::spawn(move || {
        for line in rx {
            let reply = line.and_then(|v| shim_send(&sock, &me, &v));
            let _ = net::send(
                &to_shim,
                &reply.unwrap_or_else(|e| {
                    obj(vec![("type", s("error")), ("error", s(&e.to_string()))])
                }),
            );
        }
    });
    std::thread::spawn(move || {
        for line in net::lines(Box::new(io::stdin())) {
            if tx.send(line).is_err() {
                break;
            }
        }
        // The shim's stdin closed: its harness is gone, so this session is too.
        std::process::exit(0);
    });
    serve(socket, id, store_path, hold_before_ack, Some(out))
}

fn shim_send(socket: &PathBuf, from: &str, v: &Value) -> io::Result<Value> {
    if net::field(v, "type")? != "send" {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "expected send"));
    }
    // A missing mail id is generated; a present one of the wrong type is a
    // malformed send, not a request to pick another id.
    let mail = match v.get("mail") {
        None => gen_uuid()?,
        Some(m) => m
            .as_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "mail must be a string"))?
            .to_string(),
    };
    let mail = send_mail(
        socket,
        from,
        net::field(v, "to")?,
        &mail,
        net::field(v, "body")?,
    )?;
    Ok(obj(vec![("type", s("accepted")), ("mail", s(&mail))]))
}

fn serve(
    socket: PathBuf,
    id: String,
    store_path: PathBuf,
    hold_before_ack: Option<PathBuf>,
    shim: Option<net::Writer>,
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
    match &shim {
        Some(w) => net::send(w, &obj(vec![("type", s("ready")), ("session", s(&id))]))?,
        None => {
            println!("READY session={id}");
            io::stdout().flush()?;
        }
    }

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
                        if shim.is_none() {
                            println!("STORED mail={mail} record={record}");
                            io::stdout().flush()?;
                        }
                        record
                    }
                };
                if let Some(w) = &shim {
                    net::send(
                        w,
                        &obj(vec![
                            ("type", s("mail")),
                            ("mail", s(&mail)),
                            ("from", s(net::field(&v, "from")?)),
                            ("body", s(net::field(&v, "body")?)),
                            ("record", s(&record)),
                        ]),
                    )?;
                }
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
            "acked" if shim.is_none() => {
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

/// The client: one send, reporting acceptance on stdout.
pub fn send(
    socket: PathBuf,
    from: String,
    to: String,
    mail: String,
    body: String,
) -> io::Result<()> {
    let mail = send_mail(&socket, &from, &to, &mail, &body)?;
    println!("ACCEPTED mail={mail}");
    Ok(())
}

fn send_mail(socket: &PathBuf, from: &str, to: &str, mail: &str, body: &str) -> io::Result<String> {
    let conn = UnixStream::connect(socket)?;
    let writer = net::writer(Box::new(conn.try_clone()?));
    let mut lines = net::lines(Box::new(conn));
    net::send(
        &writer,
        &obj(vec![
            ("type", s("send")),
            ("req", Value::Int(1)),
            ("mail", s(mail)),
            ("from", s(from)),
            ("to", s(to)),
            ("body", s(body)),
        ]),
    )?;
    for line in &mut lines {
        let v = line?;
        if net::field(&v, "type")? == "accepted" {
            return Ok(net::field(&v, "mail")?.to_string());
        }
    }
    Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no accepted"))
}

/// One request to the host daemon, answered by exactly one line.
fn request(socket: &PathBuf, v: &Value) -> io::Result<Value> {
    let conn = UnixStream::connect(socket)?;
    let writer = net::writer(Box::new(conn.try_clone()?));
    net::send(&writer, v)?;
    let reply = net::lines(Box::new(conn))
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no answer"))??;
    if net::field(&reply, "type")? == "error" {
        return Err(io::Error::other(
            net::field(&reply, "reason")
                .unwrap_or("unexplained")
                .to_string(),
        ));
    }
    Ok(reply)
}

fn holder_text(v: &Value) -> String {
    match (
        net::field(v, "holder"),
        v.get("remaining_ms").and_then(|x| x.as_i64()),
    ) {
        (Ok(h), Some(r)) => format!("holder={h} remaining_ms={r} "),
        _ => String::new(),
    }
}

/// The client: one claim operation (`acquire`, `renew`, or `release`) for
/// `owner` on `key`. Prints the outcome; returns false when refused.
pub fn claim(
    socket: PathBuf,
    op: &str,
    owner: String,
    key: String,
    ttl_ms: i64,
) -> io::Result<bool> {
    let reply = request(
        &socket,
        &obj(vec![
            ("type", s("claim")),
            ("req", Value::Int(1)),
            ("op", s(op)),
            ("key", s(&key)),
            ("owner", s(&owner)),
            ("ttl_ms", Value::Int(ttl_ms)),
        ]),
    )?;
    let ok = matches!(reply.get("ok"), Some(Value::Bool(true)));
    let word = match (ok, op) {
        (false, _) => "REFUSED",
        (true, "acquire") => "CLAIMED",
        (true, "renew") => "RENEWED",
        (true, _) => "RELEASED",
    };
    println!("{word} op={op} {}key={key}", holder_text(&reply));
    Ok(ok)
}

/// The client: one Board read, one line per registered session and per live
/// claim.
pub fn board(socket: PathBuf) -> io::Result<()> {
    let reply = request(
        &socket,
        &obj(vec![("type", s("board")), ("req", Value::Int(1))]),
    )?;
    let list = |name: &str| match reply.get(name) {
        Some(Value::Arr(items)) => items.clone(),
        _ => Vec::new(),
    };
    let (sessions, claims) = (list("sessions"), list("claims"));
    for v in &sessions {
        println!(
            "SESSION address={} host={}",
            net::field(v, "session")?,
            net::field(v, "host")?
        );
    }
    for v in &claims {
        println!("CLAIM {}key={}", holder_text(v), net::field(v, "key")?);
    }
    println!("BOARD sessions={} claims={}", sessions.len(), claims.len());
    Ok(())
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
