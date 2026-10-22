mod claims;
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

/// The fleet token, from the environment: a command line is world-readable
/// through `ps`, a process's environment is not.
fn token() -> io::Result<String> {
    std::env::var("ROGER_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing ROGER_TOKEN"))
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(feature = "test-hooks")]
    if let Some(marker) = arg(&args, "--hold-before-ack") {
        let _ = session::HOLD_BEFORE_ACK.set(marker.into());
    }
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    match cmd {
        "coordinator" => coordinator::run(
            &need(&args, "--bind")?,
            arg(&args, "--port-file").map(PathBuf::from),
            PathBuf::from(need(&args, "--data")?),
            token()?,
            match arg(&args, "--unregister-after-ms") {
                Some(ms) => ms.parse().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("--unregister-after-ms expects milliseconds, saw {ms}"),
                    )
                })?,
                None => coordinator::UNREGISTER_AFTER_MS,
            },
        ),
        "daemon" => daemon::run(daemon::Config {
            coordinator: need(&args, "--coordinator")?,
            token: token()?,
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
            arg(&args, "--label"),
            PathBuf::from(need(&args, "--store")?),
            arg(&args, "--declare"),
        ),
        "adapter" => session::adapter(
            PathBuf::from(need(&args, "--socket")?),
            match arg(&args, "--id") {
                Some(id) => id,
                None => session::gen_uuid()?,
            },
            arg(&args, "--label"),
            PathBuf::from(need(&args, "--store")?),
            arg(&args, "--declare"),
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
        "claim" | "renew" | "release" => {
            let op = claims::Op::parse(if cmd == "claim" { "acquire" } else { cmd })
                .expect("the match arm names only claim ops");
            let ttl_ms = match arg(&args, "--ttl-ms") {
                Some(t) => t.parse().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("--ttl-ms expects milliseconds, saw {t}"),
                    )
                })?,
                None => 0,
            };
            let granted = session::claim(
                PathBuf::from(need(&args, "--socket")?),
                &op,
                need(&args, "--owner")?,
                need(&args, "--key")?,
                ttl_ms,
            )?;
            if !granted {
                std::process::exit(3);
            }
            Ok(())
        }
        "board" => session::board(
            PathBuf::from(need(&args, "--socket")?),
            arg(&args, "--from"),
        ),
        "receipt" => session::receipt(
            PathBuf::from(need(&args, "--socket")?),
            need(&args, "--mail")?,
        ),
        "label" => session::relabel(
            PathBuf::from(need(&args, "--socket")?),
            need(&args, "--from")?,
            need(&args, "--label")?,
        ),
        _ => {
            eprintln!("usage: roger <coordinator|daemon|session|adapter|send|claim|renew|release|board|label|receipt> [flags]");
            std::process::exit(2);
        }
    }
}
