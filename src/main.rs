mod coordinator;
mod daemon;
mod durlog;
mod json;
mod net;
mod session;

use std::io;
use std::path::PathBuf;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn need(args: &[String], name: &str) -> io::Result<String> {
    arg(args, name)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("missing {name}")))
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    match cmd {
        "coordinator" => coordinator::run(
            &need(&args, "--bind")?,
            arg(&args, "--port-file").map(PathBuf::from),
            PathBuf::from(need(&args, "--data")?),
            need(&args, "--token")?,
        ),
        "daemon" => daemon::run(daemon::Config {
            coordinator: need(&args, "--coordinator")?,
            token: need(&args, "--token")?,
            host_id: need(&args, "--host-id")?,
            socket: PathBuf::from(need(&args, "--socket")?),
            data: PathBuf::from(need(&args, "--data")?),
        }),
        "session" => session::run(
            PathBuf::from(need(&args, "--socket")?),
            match arg(&args, "--id") {
                Some(id) => id,
                None => session::gen_uuid()?,
            },
            PathBuf::from(need(&args, "--store")?),
            arg(&args, "--hold-before-ack").map(PathBuf::from),
        ),
        "adapter" => session::adapter(
            PathBuf::from(need(&args, "--socket")?),
            match arg(&args, "--id") {
                Some(id) => id,
                None => session::gen_uuid()?,
            },
            PathBuf::from(need(&args, "--store")?),
            arg(&args, "--hold-before-ack").map(PathBuf::from),
        ),
        "send" => session::send(
            PathBuf::from(need(&args, "--socket")?),
            need(&args, "--from")?,
            need(&args, "--to")?,
            match arg(&args, "--mail") {
                Some(m) => m,
                None => session::gen_uuid()?,
            },
            need(&args, "--body")?,
        ),
        _ => {
            eprintln!("usage: roger <coordinator|daemon|session|adapter|send> [flags]");
            std::process::exit(2);
        }
    }
}
