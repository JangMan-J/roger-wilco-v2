//! The host daemon: one per host. Connects outbound to the coordinator,
//! holds the durable local inbox for sessions on this host, and serves
//! them over a Unix socket. Deliveries are committed to the local log
//! before they are pushed to a session; acks are committed locally, then
//! forwarded upstream. Pending mail is pushed on registration, never by
//! a timer.

use crate::durlog::DurLog;
use crate::json::{self, obj, s, Value};
use crate::net::{self, Writer};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::TcpStream;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
struct Mail {
    from: String,
    to: String,
    body: String,
}

struct State {
    log: DurLog,
    inbox: HashMap<String, Mail>,
    order: Vec<String>,
    acked: HashSet<String>,
    sessions: HashMap<String, Writer>,
    upstream: Option<Writer>,
    next_req: i64,
    waiters: HashMap<i64, mpsc::Sender<Value>>,
}

pub struct Config {
    pub coordinator: String,
    pub token: String,
    pub host_id: String,
    pub socket: PathBuf,
    pub data: PathBuf,
}

pub fn run(cfg: Config) -> io::Result<()> {
    let (log, prior) = DurLog::open(&cfg.data)?;
    let mut state = State {
        log,
        inbox: HashMap::new(),
        order: Vec::new(),
        acked: HashSet::new(),
        sessions: HashMap::new(),
        upstream: None,
        next_req: 1,
        waiters: HashMap::new(),
    };
    for line in &prior {
        if let Ok(v) = json::parse(line) {
            apply(&mut state, &v);
        }
    }
    let state = Arc::new(Mutex::new(state));
    let cfg = Arc::new(cfg);

    {
        let state = Arc::clone(&state);
        let cfg = Arc::clone(&cfg);
        std::thread::spawn(move || upstream_loop(cfg, state));
    }

    let _ = std::fs::remove_file(&cfg.socket);
    let listener = UnixListener::bind(&cfg.socket)?;
    eprintln!("host daemon listening on {}", cfg.socket.display());
    for conn in listener.incoming() {
        let conn = conn?;
        let state = Arc::clone(&state);
        std::thread::spawn(move || {
            if let Err(e) = serve_local(conn, state) {
                eprintln!("local connection ended: {e}");
            }
        });
    }
    Ok(())
}

fn apply(state: &mut State, v: &Value) {
    match v.get("ev").and_then(|x| x.as_str()) {
        Some("deliver") => {
            if let (Ok(mail), Ok(from), Ok(to), Ok(body)) = (
                net::field(v, "mail"),
                net::field(v, "from"),
                net::field(v, "to"),
                net::field(v, "body"),
            ) {
                if !state.inbox.contains_key(mail) {
                    state.order.push(mail.to_string());
                    state.inbox.insert(
                        mail.to_string(),
                        Mail {
                            from: from.into(),
                            to: to.into(),
                            body: body.into(),
                        },
                    );
                }
            }
        }
        Some("receipt") => {
            if let Ok(mail) = net::field(v, "mail") {
                state.acked.insert(mail.to_string());
            }
        }
        _ => {}
    }
}

fn deliver_msg(mail_id: &str, m: &Mail) -> Value {
    obj(vec![
        ("type", s("deliver")),
        ("mail", s(mail_id)),
        ("from", s(&m.from)),
        ("to", s(&m.to)),
        ("body", s(&m.body)),
    ])
}

fn upstream_loop(cfg: Arc<Config>, state: Arc<Mutex<State>>) {
    loop {
        match upstream_once(&cfg, &state) {
            Ok(()) => eprintln!("upstream connection closed"),
            Err(e) => eprintln!("upstream error: {e}"),
        }
        {
            let mut st = state.lock().unwrap();
            st.upstream = None;
            st.waiters.clear();
        }
        // Reconnect after failure. This never looks for work: the
        // coordinator pushes pending mail when the hello is answered.
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn upstream_once(cfg: &Config, state: &Arc<Mutex<State>>) -> io::Result<()> {
    let conn = TcpStream::connect(&cfg.coordinator)?;
    let writer = net::writer(Box::new(conn.try_clone()?));
    let mut lines = net::lines(Box::new(conn));
    net::send(
        &writer,
        &obj(vec![
            ("type", s("hello")),
            ("token", s(&cfg.token)),
            ("host", s(&cfg.host_id)),
        ]),
    )?;
    let resp = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no hello_ok"))??;
    if net::field(&resp, "type")? != "hello_ok" {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "hello refused",
        ));
    }
    let known: Vec<String> = {
        let mut st = state.lock().unwrap();
        st.upstream = Some(Arc::clone(&writer));
        st.sessions.keys().cloned().collect()
    };
    for session in known {
        let _ = net::send(
            &writer,
            &obj(vec![("type", s("register")), ("session", s(&session))]),
        );
    }
    for line in lines {
        let v = line?;
        match net::field(&v, "type")? {
            "deliver" => handle_deliver(state, &v)?,
            "accepted" | "acked" | "error" => {
                if let Some(req) = v.get("req").and_then(|x| x.as_i64()) {
                    let waiter = state.lock().unwrap().waiters.remove(&req);
                    if let Some(tx) = waiter {
                        let _ = tx.send(v);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn handle_deliver(state: &Arc<Mutex<State>>, v: &Value) -> io::Result<()> {
    let mail_id = net::field(v, "mail")?.to_string();
    let mut push: Option<(Writer, Value)> = None;
    {
        let mut st = state.lock().unwrap();
        if !st.inbox.contains_key(&mail_id) {
            let m = Mail {
                from: net::field(v, "from")?.into(),
                to: net::field(v, "to")?.into(),
                body: net::field(v, "body")?.into(),
            };
            let ev = obj(vec![
                ("ev", s("deliver")),
                ("mail", s(&mail_id)),
                ("from", s(&m.from)),
                ("to", s(&m.to)),
                ("body", s(&m.body)),
            ]);
            st.log.append(&json::stringify(&ev))?;
            st.order.push(mail_id.clone());
            st.inbox.insert(mail_id.clone(), m);
        }
        if !st.acked.contains(&mail_id) {
            let m = st.inbox[&mail_id].clone();
            if let Some(w) = st.sessions.get(&m.to) {
                push = Some((Arc::clone(w), deliver_msg(&mail_id, &m)));
            }
        }
    }
    if let Some((w, msg)) = push {
        let _ = net::send(&w, &msg);
    }
    Ok(())
}

fn upstream_request(state: &Arc<Mutex<State>>, mut v: Value) -> io::Result<Value> {
    let (writer, rx) = {
        let mut st = state.lock().unwrap();
        let req = st.next_req;
        st.next_req += 1;
        if let Value::Obj(pairs) = &mut v {
            pairs.push(("req".to_string(), Value::Int(req)));
        }
        let writer = st
            .upstream
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "no upstream"))?;
        let (tx, rx) = mpsc::channel();
        st.waiters.insert(req, tx);
        (writer, rx)
    };
    net::send(&writer, &v)?;
    rx.recv()
        .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "upstream wait dropped"))
}

fn serve_local(conn: UnixStream, state: Arc<Mutex<State>>) -> io::Result<()> {
    let writer = net::writer(Box::new(conn.try_clone()?));
    let lines = net::lines(Box::new(conn));
    let mut session_id: Option<String> = None;

    for line in lines {
        let v = line?;
        match net::field(&v, "type")? {
            "register" => {
                let session = net::field(&v, "session")?.to_string();
                session_id = Some(session.clone());
                let (upstream, pending) = {
                    let mut st = state.lock().unwrap();
                    st.sessions.insert(session.clone(), Arc::clone(&writer));
                    let pending: Vec<Value> = st
                        .order
                        .iter()
                        .filter(|id| !st.acked.contains(*id) && st.inbox[*id].to == session)
                        .map(|id| deliver_msg(id, &st.inbox[id]))
                        .collect();
                    (st.upstream.clone(), pending)
                };
                if let Some(up) = upstream {
                    let _ = net::send(
                        &up,
                        &obj(vec![("type", s("register")), ("session", s(&session))]),
                    );
                }
                for p in pending {
                    let _ = net::send(&writer, &p);
                }
            }
            "send" => {
                let req = v.get("req").and_then(|x| x.as_i64()).unwrap_or(0);
                let resp = upstream_request(
                    &state,
                    obj(vec![
                        ("type", s("send")),
                        ("mail", s(net::field(&v, "mail")?)),
                        ("from", s(net::field(&v, "from")?)),
                        ("to", s(net::field(&v, "to")?)),
                        ("body", s(net::field(&v, "body")?)),
                    ]),
                )?;
                net::send(
                    &writer,
                    &obj(vec![
                        ("type", s("accepted")),
                        ("req", Value::Int(req)),
                        ("mail", s(net::field(&resp, "mail")?)),
                    ]),
                )?;
            }
            "ack" => {
                let req = v.get("req").and_then(|x| x.as_i64()).unwrap_or(0);
                let session = session_id.clone().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "ack before register")
                })?;
                let mail = net::field(&v, "mail")?.to_string();
                let record = net::field(&v, "record")?.to_string();
                {
                    let mut st = state.lock().unwrap();
                    if !st.acked.contains(&mail) {
                        let ev = obj(vec![
                            ("ev", s("receipt")),
                            ("mail", s(&mail)),
                            ("session", s(&session)),
                            ("record", s(&record)),
                        ]);
                        st.log.append(&json::stringify(&ev))?;
                        st.acked.insert(mail.clone());
                    }
                }
                upstream_request(
                    &state,
                    obj(vec![
                        ("type", s("ack")),
                        ("mail", s(&mail)),
                        ("session", s(&session)),
                        ("record", s(&record)),
                    ]),
                )?;
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

    if let Some(id) = session_id {
        let mut st = state.lock().unwrap();
        st.sessions.remove(&id);
    }
    Ok(())
}
