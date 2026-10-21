//! The coordinator: single central durable store for sessions, mail, and
//! receipts. Host daemons connect to it; committed mail is pushed to the
//! recipient's host daemon, and pending mail is pushed when a session or
//! host (re)registers. Nothing here scans on a timer.

use crate::durlog::DurLog;
use crate::json::{self, obj, s, Value};
use crate::net::{self, Writer};
use std::collections::HashMap;
use std::io;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Mail {
    from: String,
    to: String,
    body: String,
    acked: bool,
}

struct State {
    log: DurLog,
    sessions: HashMap<String, String>, // session -> host
    mails: HashMap<String, Mail>,
    order: Vec<String>,
    hosts: HashMap<String, (u64, Writer)>, // host -> (connection id, live connection)
    next_conn: u64,
}

pub fn run(bind: &str, port_file: Option<PathBuf>, data: PathBuf, token: String) -> io::Result<()> {
    let (log, prior) = DurLog::open(&data)?;
    let mut state = State {
        log,
        sessions: HashMap::new(),
        mails: HashMap::new(),
        order: Vec::new(),
        hosts: HashMap::new(),
        next_conn: 1,
    };
    for line in &prior {
        if let Ok(v) = json::parse(line) {
            apply(&mut state, &v);
        }
    }
    let state = Arc::new(Mutex::new(state));

    let listener = TcpListener::bind(bind)?;
    if let Some(pf) = &port_file {
        std::fs::write(pf, listener.local_addr()?.port().to_string())?;
    }
    eprintln!("coordinator listening on {}", listener.local_addr()?);

    for conn in listener.incoming() {
        let conn = conn?;
        let state = Arc::clone(&state);
        let token = token.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(conn, state, &token) {
                eprintln!("coordinator connection ended: {e}");
            }
        });
    }
    Ok(())
}

fn apply(state: &mut State, v: &Value) {
    match v.get("ev").and_then(|x| x.as_str()) {
        Some("session") => {
            if let (Ok(session), Ok(host)) = (net::field(v, "session"), net::field(v, "host")) {
                state.sessions.insert(session.to_string(), host.to_string());
            }
        }
        Some("mail") => {
            if let (Ok(mail), Ok(from), Ok(to), Ok(body)) = (
                net::field(v, "mail"),
                net::field(v, "from"),
                net::field(v, "to"),
                net::field(v, "body"),
            ) {
                state.order.push(mail.to_string());
                state.mails.insert(
                    mail.to_string(),
                    Mail {
                        from: from.into(),
                        to: to.into(),
                        body: body.into(),
                        acked: false,
                    },
                );
            }
        }
        Some("receipt") => {
            if let Ok(mail) = net::field(v, "mail") {
                if let Some(m) = state.mails.get_mut(mail) {
                    m.acked = true;
                }
            }
        }
        _ => {}
    }
}

/// Pending deliveries for sessions on `host`, or for one session.
fn pending_pushes(state: &State, host: &str, only_session: Option<&str>) -> Vec<Value> {
    let mut out = Vec::new();
    for id in &state.order {
        let m = &state.mails[id];
        if m.acked {
            continue;
        }
        if let Some(want) = only_session {
            if m.to != want {
                continue;
            }
        } else if state.sessions.get(&m.to).map(|h| h.as_str()) != Some(host) {
            continue;
        }
        out.push(net::deliver_msg(id, &m.from, &m.to, &m.body));
    }
    out
}

fn serve(conn: std::net::TcpStream, state: Arc<Mutex<State>>, token: &str) -> io::Result<()> {
    let writer = net::writer(Box::new(conn.try_clone()?));
    let mut lines = net::lines(Box::new(conn));

    let hello = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no hello"))??;
    if net::field(&hello, "type")? != "hello" || net::field(&hello, "token")? != token {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "bad hello"));
    }
    let host = net::field(&hello, "host")?.to_string();
    let conn_id = {
        let mut st = state.lock().unwrap();
        let id = st.next_conn;
        st.next_conn += 1;
        st.hosts.insert(host.clone(), (id, Arc::clone(&writer)));
        id
    };
    net::send(&writer, &obj(vec![("type", s("hello_ok"))]))?;
    let pushes = {
        let st = state.lock().unwrap();
        pending_pushes(&st, &host, None)
    };
    for p in pushes {
        let _ = net::send(&writer, &p);
    }

    for line in lines {
        let v = line?;
        match net::field(&v, "type")? {
            "register" => {
                let session = net::field(&v, "session")?.to_string();
                let pushes = {
                    let mut st = state.lock().unwrap();
                    if st.sessions.get(&session).map(|h| h.as_str()) != Some(host.as_str()) {
                        let ev = obj(vec![
                            ("ev", s("session")),
                            ("session", s(&session)),
                            ("host", s(&host)),
                        ]);
                        st.log.append(&json::stringify(&ev))?;
                        st.sessions.insert(session.clone(), host.clone());
                    }
                    pending_pushes(&st, &host, Some(&session))
                };
                for p in pushes {
                    let _ = net::send(&writer, &p);
                }
            }
            "send" => {
                let req = v.get("req").and_then(|x| x.as_i64()).unwrap_or(0);
                let mail = net::field(&v, "mail")?.to_string();
                let mut push: Option<(Writer, Value)> = None;
                {
                    let mut st = state.lock().unwrap();
                    if !st.mails.contains_key(&mail) {
                        let m = Mail {
                            from: net::field(&v, "from")?.into(),
                            to: net::field(&v, "to")?.into(),
                            body: net::field(&v, "body")?.into(),
                            acked: false,
                        };
                        let ev = obj(vec![
                            ("ev", s("mail")),
                            ("mail", s(&mail)),
                            ("from", s(&m.from)),
                            ("to", s(&m.to)),
                            ("body", s(&m.body)),
                        ]);
                        st.log.append(&json::stringify(&ev))?;
                        if let Some(host_of) = st.sessions.get(&m.to).cloned() {
                            if let Some((_, w)) = st.hosts.get(&host_of) {
                                push = Some((
                                    Arc::clone(w),
                                    net::deliver_msg(&mail, &m.from, &m.to, &m.body),
                                ));
                            }
                        }
                        st.order.push(mail.clone());
                        st.mails.insert(mail.clone(), m);
                    }
                }
                net::send(
                    &writer,
                    &obj(vec![
                        ("type", s("accepted")),
                        ("req", Value::Int(req)),
                        ("mail", s(&mail)),
                    ]),
                )?;
                if let Some((w, msg)) = push {
                    let _ = net::send(&w, &msg);
                }
            }
            "ack" => {
                let req = v.get("req").and_then(|x| x.as_i64()).unwrap_or(0);
                let mail = net::field(&v, "mail")?.to_string();
                let session = net::field(&v, "session")?.to_string();
                let record = net::field(&v, "record")?.to_string();
                {
                    let mut st = state.lock().unwrap();
                    let needs = st.mails.get(&mail).map(|m| !m.acked).unwrap_or(false);
                    if needs {
                        let ev = obj(vec![
                            ("ev", s("receipt")),
                            ("mail", s(&mail)),
                            ("session", s(&session)),
                            ("record", s(&record)),
                        ]);
                        st.log.append(&json::stringify(&ev))?;
                        if let Some(m) = st.mails.get_mut(&mail) {
                            m.acked = true;
                        }
                    }
                }
                net::send(
                    &writer,
                    &obj(vec![
                        ("type", s("acked")),
                        ("req", Value::Int(req)),
                        ("mail", s(&mail)),
                        ("record", s(&record)),
                    ]),
                )?;
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown type {other}"),
                ))
            }
        }
    }

    let mut st = state.lock().unwrap();
    // Only this connection may take the slot: a host that reconnected while
    // this thread was finishing already owns it.
    if st.hosts.get(&host).map(|(id, _)| *id) == Some(conn_id) {
        st.hosts.remove(&host);
    }
    Ok(())
}
