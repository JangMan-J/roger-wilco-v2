//! The coordinator: single central durable store for sessions, mail, and
//! receipts. Host daemons connect to it; committed mail is pushed to the
//! recipient's host daemon, and pending mail is pushed when a session or
//! host (re)registers. Claims are decided against their committed deadlines
//! at the moment of each request or board read. Nothing here scans on a timer.

use crate::claims::{Change, Claims, Held, Op};
use crate::durlog::DurLog;
use crate::json::{self, obj, s, Value};
use crate::net::{self, Writer};
use std::collections::{HashMap, HashSet};
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
    /// False only for a Broadcast copy: a broadcast makes no wake request.
    wake: bool,
}

/// The Address a send names to reach every registered session at once.
pub const BROADCAST: &str = "*";

struct State {
    log: DurLog,
    sessions: HashMap<String, String>, // session -> host
    labels: HashMap<String, String>,   // session -> label, stored verbatim
    mails: HashMap<String, Mail>,
    order: Vec<String>,
    /// Broadcast ids already committed, so a resent broadcast fans out once.
    broadcasts: HashSet<String>,
    hosts: HashMap<String, (u64, Writer)>, // host -> (connection id, live connection)
    next_conn: u64,
    claims: Claims,
}

/// Coordinator wall clock in milliseconds since the Unix epoch: the one clock
/// every claim deadline is set and read against, whichever host asks.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

pub fn run(bind: &str, port_file: Option<PathBuf>, data: PathBuf, token: String) -> io::Result<()> {
    let (log, prior) = DurLog::open(&data)?;
    let mut state = State {
        log,
        sessions: HashMap::new(),
        labels: HashMap::new(),
        mails: HashMap::new(),
        order: Vec::new(),
        broadcasts: HashSet::new(),
        hosts: HashMap::new(),
        next_conn: 1,
        claims: Claims::default(),
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
        Some("label") => {
            if let (Ok(session), Ok(label)) = (net::field(v, "session"), net::field(v, "label")) {
                state.labels.insert(session.to_string(), label.to_string());
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
                        wake: true,
                    },
                );
            }
        }
        Some("broadcast") => {
            if let (Ok(mail), Ok(from), Ok(body), Some(Value::Arr(to))) = (
                net::field(v, "mail"),
                net::field(v, "from"),
                net::field(v, "body"),
                v.get("to"),
            ) {
                let to: Vec<String> = to
                    .iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect();
                fan_out(state, mail, from, body, &to);
            }
        }
        Some("receipt") => {
            if let Ok(mail) = net::field(v, "mail") {
                if let Some(m) = state.mails.get_mut(mail) {
                    m.acked = true;
                }
            }
        }
        Some("claim") => {
            if let (Ok(key), Ok(owner), Some(expires_ms)) = (
                net::field(v, "key"),
                net::field(v, "owner"),
                v.get("expires_ms").and_then(|x| x.as_i64()),
            ) {
                let change = Change::Grant {
                    owner: owner.to_string(),
                    expires_ms,
                };
                state.claims.apply(key, &change);
            }
        }
        Some("release") => {
            if let Ok(key) = net::field(v, "key") {
                state.claims.apply(key, &Change::Release);
            }
        }
        _ => {}
    }
}

/// Record a session's label verbatim. The label is display only: it never
/// touches the session's Address, host, or pending mail.
fn set_label(state: &mut State, session: &str, label: &str) -> io::Result<()> {
    let ev = obj(vec![
        ("ev", s("label")),
        ("session", s(session)),
        ("label", s(label)),
    ]);
    state.log.append(&json::stringify(&ev))?;
    state.labels.insert(session.to_string(), label.to_string());
    Ok(())
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
        out.push(net::deliver_msg(id, &m.from, &m.to, &m.body, m.wake));
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
                    if let Some(label) = v.get("label").and_then(|x| x.as_str()) {
                        set_label(&mut st, &session, label)?;
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
                let mut push: Vec<(Writer, Value)> = Vec::new();
                {
                    let mut st = state.lock().unwrap();
                    if net::field(&v, "to")? == BROADCAST {
                        push = broadcast(
                            &mut st,
                            &mail,
                            net::field(&v, "from")?,
                            net::field(&v, "body")?,
                        )?;
                    } else if !st.mails.contains_key(&mail) {
                        let m = Mail {
                            from: net::field(&v, "from")?.into(),
                            to: net::field(&v, "to")?.into(),
                            body: net::field(&v, "body")?.into(),
                            acked: false,
                            wake: true,
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
                                push.push((
                                    Arc::clone(w),
                                    net::deliver_msg(&mail, &m.from, &m.to, &m.body, true),
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
                for (w, msg) in push {
                    let _ = net::send(&w, &msg);
                }
            }
            "relabel" => {
                let req = v.get("req").and_then(|x| x.as_i64()).unwrap_or(0);
                let session = net::field(&v, "session")?.to_string();
                let label = net::field(&v, "label")?.to_string();
                set_label(&mut state.lock().unwrap(), &session, &label)?;
                net::send(
                    &writer,
                    &obj(vec![
                        ("type", s("relabeled")),
                        ("req", Value::Int(req)),
                        ("session", s(&session)),
                        ("label", s(&label)),
                    ]),
                )?;
            }
            "ack" => {
                let req = v.get("req").and_then(|x| x.as_i64()).unwrap_or(0);
                let mail = net::field(&v, "mail")?.to_string();
                let session = net::field(&v, "session")?.to_string();
                let record = net::field(&v, "record")?.to_string();
                let known = {
                    let mut st = state.lock().unwrap();
                    match st.mails.get(&mail) {
                        None => None,
                        Some(m) => {
                            let already = m.acked;
                            if !already {
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
                            Some(already)
                        }
                    }
                };
                // Answering `acked` for a mail this coordinator never accepted
                // would be a false confirmation: the host daemon trims that
                // receipt from its replay list on the answer, and the receipt
                // would never be stored upstream.
                let reply = match known {
                    Some(_) => obj(vec![
                        ("type", s("acked")),
                        ("req", Value::Int(req)),
                        ("mail", s(&mail)),
                        ("record", s(&record)),
                    ]),
                    None => obj(vec![
                        ("type", s("error")),
                        ("req", Value::Int(req)),
                        ("mail", s(&mail)),
                        ("reason", s("unknown mail")),
                    ]),
                };
                net::send(&writer, &reply)?;
            }
            "claim" => {
                let reply = {
                    let mut st = state.lock().unwrap();
                    claim(&mut st, &v)?
                };
                net::send(&writer, &reply)?;
            }
            "board" => {
                let reply = {
                    let st = state.lock().unwrap();
                    board(&st, &v)
                };
                net::send(&writer, &reply)?;
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

fn holder_fields(pairs: &mut Vec<(&str, Value)>, holder: Option<&Held>, now: i64) {
    if let Some(h) = holder {
        pairs.push(("holder", s(&h.owner)));
        pairs.push(("remaining_ms", Value::Int(h.expires_ms - now)));
    }
}

/// Decide one claim request, commit its change durably, then answer. The lock
/// is held throughout, so two owners racing for one key see one order.
fn claim(st: &mut State, v: &Value) -> io::Result<Value> {
    let req = v.get("req").and_then(|x| x.as_i64()).unwrap_or(0);
    let op_name = net::field(v, "op")?;
    let op = Op::parse(op_name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected op acquire, renew, or release; saw {op_name}"),
        )
    })?;
    let key = net::field(v, "key")?.to_string();
    let owner = net::field(v, "owner")?.to_string();
    let ttl_ms = v.get("ttl_ms").and_then(|x| x.as_i64()).unwrap_or(0);
    if op != Op::Release && ttl_ms <= 0 {
        return Ok(obj(vec![
            ("type", s("error")),
            ("req", Value::Int(req)),
            (
                "reason",
                s(&format!("ttl_ms must be positive, saw {ttl_ms}")),
            ),
        ]));
    }
    let now = now_ms();
    let d = st.claims.decide(&op, &key, &owner, ttl_ms, now);
    if let Some(change) = &d.change {
        let ev = match change {
            Change::Grant { owner, expires_ms } => obj(vec![
                ("ev", s("claim")),
                ("key", s(&key)),
                ("owner", s(owner)),
                ("expires_ms", Value::Int(*expires_ms)),
            ]),
            Change::Release => obj(vec![
                ("ev", s("release")),
                ("key", s(&key)),
                ("owner", s(&owner)),
            ]),
        };
        st.log.append(&json::stringify(&ev))?;
        st.claims.apply(&key, change);
    }
    let mut pairs = vec![
        ("type", s("claim_result")),
        ("req", Value::Int(req)),
        ("op", s(op.name())),
        ("key", s(&key)),
        ("ok", Value::Bool(d.ok)),
    ];
    holder_fields(&mut pairs, d.holder.as_ref(), now);
    Ok(obj(pairs))
}

/// The Board: registered sessions and every claim live at the moment of the
/// read.
fn board(st: &State, v: &Value) -> Value {
    let req = v.get("req").and_then(|x| x.as_i64()).unwrap_or(0);
    let now = now_ms();
    let mut sessions: Vec<(&String, &String)> = st.sessions.iter().collect();
    sessions.sort();
    let sessions = sessions
        .into_iter()
        .map(|(session, host)| {
            obj(vec![
                ("session", s(session)),
                ("host", s(host)),
                (
                    "label",
                    s(st.labels.get(session).map(String::as_str).unwrap_or("")),
                ),
            ])
        })
        .collect();
    let claims = st
        .claims
        .active(now)
        .into_iter()
        .map(|(key, h)| {
            let mut pairs = vec![("key", s(key))];
            holder_fields(&mut pairs, Some(h), now);
            obj(pairs)
        })
        .collect();
    obj(vec![
        ("type", s("board")),
        ("req", Value::Int(req)),
        ("sessions", Value::Arr(sessions)),
        ("claims", Value::Arr(claims)),
    ])
}

/// The Mail id of one Broadcast copy: the broadcast's id and the recipient's
/// Address, so each recipient's copy is delivered, stored, and acked once.
fn copy_id(mail: &str, to: &str) -> String {
    format!("{mail}/{to}")
}

/// Expand a committed Broadcast into one Mail per recipient, carrying no wake
/// request. Used on commit and on replay, so both see the same copies.
fn fan_out(st: &mut State, mail: &str, from: &str, body: &str, to: &[String]) {
    if !st.broadcasts.insert(mail.to_string()) {
        return;
    }
    for addr in to {
        let id = copy_id(mail, addr);
        if st.mails.contains_key(&id) {
            continue;
        }
        st.order.push(id.clone());
        st.mails.insert(
            id,
            Mail {
                from: from.to_string(),
                to: addr.clone(),
                body: body.to_string(),
                acked: false,
                wake: false,
            },
        );
    }
}

/// Commit one Broadcast to every session registered at this moment, as a
/// single durable event, then return the deliveries to push to connected
/// host daemons. A resent broadcast id commits nothing.
fn broadcast(
    st: &mut State,
    mail: &str,
    from: &str,
    body: &str,
) -> io::Result<Vec<(Writer, Value)>> {
    if st.broadcasts.contains(mail) {
        return Ok(Vec::new());
    }
    let mut to: Vec<String> = st.sessions.keys().cloned().collect();
    to.sort();
    let ev = obj(vec![
        ("ev", s("broadcast")),
        ("mail", s(mail)),
        ("from", s(from)),
        ("body", s(body)),
        ("to", Value::Arr(to.iter().map(|a| s(a)).collect())),
    ]);
    st.log.append(&json::stringify(&ev))?;
    fan_out(st, mail, from, body, &to);
    let mut push = Vec::new();
    for addr in &to {
        let host = st.sessions.get(addr);
        if let Some((_, w)) = host.and_then(|h| st.hosts.get(h)) {
            push.push((
                Arc::clone(w),
                net::deliver_msg(&copy_id(mail, addr), from, addr, body, false),
            ));
        }
    }
    Ok(push)
}
