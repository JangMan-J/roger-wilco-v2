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

/// A receipt committed to this host's log: which Mail, which session acked
/// it, and which Record id that session named.
#[derive(Clone, PartialEq)]
struct Receipt {
    mail: String,
    session: String,
    record: String,
}

struct State {
    log: DurLog,
    inbox: HashMap<String, Mail>,
    order: Vec<String>,
    acked: HashSet<String>,
    /// Receipts committed locally and not yet confirmed upstream, in commit
    /// order. Replayed after every reconnect, because the coordinator's ack
    /// handler is idempotent and a locally committed receipt that never
    /// arrives upstream would leave the mail pending forever.
    receipts: Vec<Receipt>,
    sessions: HashMap<String, Writer>,
    upstream: Option<Writer>,
    next_req: i64,
    waiters: HashMap<i64, mpsc::Sender<Value>>,
}

/// How long a session's send or ack waits for the coordinator to answer.
const UPSTREAM_WAIT: Duration = Duration::from_secs(10);

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
        receipts: Vec::new(),
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
            if let (Ok(mail), Ok(session), Ok(record)) = (
                net::field(v, "mail"),
                net::field(v, "session"),
                net::field(v, "record"),
            ) {
                if state.acked.insert(mail.to_string()) {
                    state.receipts.push(Receipt {
                        mail: mail.to_string(),
                        session: session.to_string(),
                        record: record.to_string(),
                    });
                }
            }
        }
        _ => {}
    }
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
        // no-poll: reconnect backoff after a failed connection; it never looks for work.
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
    let (known, receipts): (Vec<Value>, Vec<Receipt>) = {
        let mut st = state.lock().unwrap();
        st.upstream = Some(Arc::clone(&writer));
        // Labels are not repeated here: the coordinator's log already holds
        // the latest one, and a relabel may have gone through another host.
        let known = st
            .sessions
            .keys()
            .map(|id| register_msg(id, None))
            .collect();
        (known, st.receipts.clone())
    };
    for msg in known {
        let _ = net::send(&writer, &msg);
    }
    // A receipt committed while upstream was down still has to arrive. The
    // coordinator's acked answers trim `receipts` in the loop below.
    if !receipts.is_empty() {
        eprintln!("upstream connected, replaying {} receipts", receipts.len());
    }
    for r in &receipts {
        let _ = net::send(&writer, &net::ack_msg(&r.mail, &r.session, &r.record));
    }
    for line in lines {
        let v = line?;
        match net::field(&v, "type")? {
            "deliver" => handle_deliver(state, &v)?,
            "acked" => {
                let waiter = v
                    .get("req")
                    .and_then(|x| x.as_i64())
                    .and_then(|req| state.lock().unwrap().waiters.remove(&req));
                if let Some(tx) = waiter {
                    let _ = tx.send(v.clone());
                }
                // Upstream has this receipt now, so it leaves the replay list.
                if let Ok(mail) = net::field(&v, "mail") {
                    let mail = mail.to_string();
                    state.lock().unwrap().receipts.retain(|r| r.mail != mail);
                }
            }
            "error" => {
                let waiter = v
                    .get("req")
                    .and_then(|x| x.as_i64())
                    .and_then(|req| state.lock().unwrap().waiters.remove(&req));
                if let Some(tx) = waiter {
                    let _ = tx.send(v.clone());
                } else if let Ok(mail) = net::field(&v, "mail") {
                    // A replayed receipt upstream refuses stays queued, so say
                    // so: silence here would look like a confirmed receipt.
                    eprintln!(
                        "upstream refused the replayed receipt for {mail}: {}",
                        net::field(&v, "reason").unwrap_or("unexplained")
                    );
                }
            }
            "accepted" | "claim_result" | "board" | "relabeled" => {
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

fn register_msg(session: &str, label: Option<&str>) -> Value {
    let mut pairs = vec![("type", s("register")), ("session", s(session))];
    if let Some(label) = label {
        pairs.push(("label", s(label)));
    }
    obj(pairs)
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
                push = Some((
                    Arc::clone(w),
                    net::deliver_msg(&mail_id, &m.from, &m.to, &m.body),
                ));
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
    // Bounded: a coordinator that accepts a request and never answers must
    // not hang the session's send or ack forever.
    // no-poll: one bounded wait for a single reply, turning a silent coordinator into an error.
    rx.recv_timeout(UPSTREAM_WAIT).map_err(|e| match e {
        mpsc::RecvTimeoutError::Timeout => {
            io::Error::new(io::ErrorKind::TimedOut, "upstream did not answer")
        }
        mpsc::RecvTimeoutError::Disconnected => {
            io::Error::new(io::ErrorKind::UnexpectedEof, "upstream wait dropped")
        }
    })
}

/// An upstream reply, re-tagged with the local request number it answers.
fn with_req(mut v: Value, req: i64) -> Value {
    if let Value::Obj(pairs) = &mut v {
        pairs.retain(|(k, _)| k != "req");
        pairs.push(("req".to_string(), Value::Int(req)));
    }
    v
}

fn serve_local(conn: UnixStream, state: Arc<Mutex<State>>) -> io::Result<()> {
    let writer = net::writer(Box::new(conn.try_clone()?));
    let lines = net::lines(Box::new(conn));
    let mut session_id: Option<String> = None;
    let result = local_loop(&writer, lines, &state, &mut session_id);
    // Whatever ended the connection, a dead Writer must not stay registered:
    // the daemon would re-register that session upstream and push pending
    // mail to a closed socket.
    if let Some(id) = session_id {
        let mut st = state.lock().unwrap();
        st.sessions.remove(&id);
    }
    result
}

fn local_loop(
    writer: &Writer,
    lines: net::Lines,
    state: &Arc<Mutex<State>>,
    session_id: &mut Option<String>,
) -> io::Result<()> {
    for line in lines {
        let v = line?;
        match net::field(&v, "type")? {
            "register" => {
                let session = net::field(&v, "session")?.to_string();
                let label = v.get("label").and_then(|x| x.as_str()).map(str::to_string);
                *session_id = Some(session.clone());
                let (upstream, pending) = {
                    let mut st = state.lock().unwrap();
                    st.sessions.insert(session.clone(), Arc::clone(writer));
                    let pending: Vec<Value> = st
                        .order
                        .iter()
                        .filter(|id| !st.acked.contains(*id) && st.inbox[*id].to == session)
                        .map(|id| {
                            net::deliver_msg(
                                id,
                                &st.inbox[id].from,
                                &st.inbox[id].to,
                                &st.inbox[id].body,
                            )
                        })
                        .collect();
                    (st.upstream.clone(), pending)
                };
                if let Some(up) = upstream {
                    let _ = net::send(&up, &register_msg(&session, label.as_deref()));
                }
                for p in pending {
                    let _ = net::send(writer, &p);
                }
            }
            "send" => {
                let req = v.get("req").and_then(|x| x.as_i64()).unwrap_or(0);
                let resp = upstream_request(
                    state,
                    obj(vec![
                        ("type", s("send")),
                        ("mail", s(net::field(&v, "mail")?)),
                        ("from", s(net::field(&v, "from")?)),
                        ("to", s(net::field(&v, "to")?)),
                        ("body", s(net::field(&v, "body")?)),
                    ]),
                )?;
                net::send(
                    writer,
                    &obj(vec![
                        ("type", s("accepted")),
                        ("req", Value::Int(req)),
                        ("mail", s(net::field(&resp, "mail")?)),
                    ]),
                )?;
            }
            "relabel" => {
                let req = v.get("req").and_then(|x| x.as_i64()).unwrap_or(0);
                let session = net::field(&v, "session")?.to_string();
                let label = net::field(&v, "label")?.to_string();
                let resp = upstream_request(
                    state,
                    obj(vec![
                        ("type", s("relabel")),
                        ("session", s(&session)),
                        ("label", s(&label)),
                    ]),
                )?;
                net::send(writer, &with_req(resp, req))?;
            }
            "ack" => {
                let req = v.get("req").and_then(|x| x.as_i64()).unwrap_or(0);
                let session = (*session_id).clone().ok_or_else(|| {
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
                        // Durable first: a receipt that is acked in memory but
                        // absent from the log would never be replayed, and a
                        // retry would not commit it either.
                        st.log.append(&json::stringify(&ev))?;
                        st.acked.insert(mail.clone());
                        st.receipts.push(Receipt {
                            mail: mail.clone(),
                            session: session.clone(),
                            record: record.clone(),
                        });
                    }
                }
                // Forwarding is retryable, not part of the ack: the Receipt is
                // already durable in this host daemon, which is core, so the
                // session is answered now and the receipt is replayed after
                // the next hello_ok if this forward fails.
                match upstream_request(state, net::ack_msg(&mail, &session, &record)) {
                    Err(e) => {
                        eprintln!("receipt {mail} committed, upstream forward deferred: {e}")
                    }
                    // A refusal means upstream never accepted that mail, so the
                    // receipt stays queued and is replayed after the next
                    // hello_ok rather than being treated as confirmed.
                    Ok(resp) if net::field(&resp, "type").unwrap_or("") == "error" => eprintln!(
                        "receipt {mail} committed, upstream refused it: {}",
                        net::field(&resp, "reason").unwrap_or("unexplained")
                    ),
                    Ok(_) => {}
                }
                net::send(
                    writer,
                    &obj(vec![
                        ("type", s("acked")),
                        ("req", Value::Int(req)),
                        ("mail", s(&mail)),
                        ("record", s(&record)),
                    ]),
                )?;
            }
            // Claims and the Board live only at the coordinator, so the host
            // daemon relays them and answers under the session's own req.
            "claim" | "board" => {
                let req = v.get("req").and_then(|x| x.as_i64()).unwrap_or(0);
                let mut upstream = v.clone();
                if let Value::Obj(pairs) = &mut upstream {
                    pairs.retain(|(k, _)| k != "req");
                }
                let mut resp = upstream_request(state, upstream)?;
                if let Value::Obj(pairs) = &mut resp {
                    for (k, val) in pairs.iter_mut() {
                        if k == "req" {
                            *val = Value::Int(req);
                        }
                    }
                }
                net::send(writer, &resp)?;
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown type {other}"),
                ))
            }
        }
    }

    Ok(())
}
