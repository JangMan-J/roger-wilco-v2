//! Process-level seam: real coordinator, daemon, and session processes on
//! loopback. Expected values are the literals chosen here, per proof 1.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_roger");
const TOKEN: &str = "process-test-token";
const SENDER: &str = "11111111-1111-4111-8111-111111111111";
const RECIPIENT: &str = "22222222-2222-4222-8222-222222222222";

struct Rig {
    dir: PathBuf,
    children: Vec<Child>,
    coord_port: String,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let dir = std::env::temp_dir().join(format!("roger-process-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Rig {
            dir,
            children: Vec::new(),
            coord_port: String::new(),
        }
    }

    fn p(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn spawn(&mut self, args: &[&str], stdout_name: &str) -> usize {
        let out = fs::File::create(self.p(stdout_name)).unwrap();
        let child = Command::new(BIN)
            .args(args)
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(
                fs::File::create(self.p(&format!("{stdout_name}.err"))).unwrap(),
            ))
            .spawn()
            .unwrap();
        self.children.push(child);
        self.children.len() - 1
    }

    fn start_stack(&mut self) {
        let port_file = self.p("coord.port");
        self.spawn(
            &[
                "coordinator",
                "--bind",
                "127.0.0.1:0",
                "--port-file",
                port_file.to_str().unwrap(),
                "--data",
                self.p("coord.log").to_str().unwrap(),
                "--token",
                TOKEN,
            ],
            "coord.out",
        );
        wait_for_file(&port_file, Duration::from_secs(10));
        let port = fs::read_to_string(&port_file).unwrap().trim().to_string();
        self.coord_port = port.clone();
        let coord = format!("127.0.0.1:{port}");
        for (host, sock, data, out) in [
            ("host-a", "a.sock", "a.log", "daemon-a.out"),
            ("host-b", "b.sock", "b.log", "daemon-b.out"),
        ] {
            self.spawn(
                &[
                    "daemon",
                    "--coordinator",
                    &coord,
                    "--token",
                    TOKEN,
                    "--host-id",
                    host,
                    "--socket",
                    self.p(sock).to_str().unwrap(),
                    "--data",
                    self.p(data).to_str().unwrap(),
                ],
                out,
            );
            wait_for_file(&self.p(sock), Duration::from_secs(10));
        }
    }

    fn start_recipient(&mut self, out: &str, hold: bool) -> usize {
        let mut args: Vec<String> = vec![
            "session".to_string(),
            "--socket".to_string(),
            self.p("b.sock").to_str().unwrap().to_string(),
            "--id".to_string(),
            RECIPIENT.to_string(),
            "--store".to_string(),
            self.p("recipient.store").to_str().unwrap().to_string(),
        ];
        if hold {
            args.push("--hold-before-ack".to_string());
            args.push(self.p("held.marker").to_str().unwrap().to_string());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let idx = self.spawn(&arg_refs, out);
        wait_for_text(&self.p(out), "READY", Duration::from_secs(10));
        idx
    }

    fn send(&self, mail: &str, body: &str) -> String {
        let output = Command::new(BIN)
            .args([
                "send",
                "--socket",
                self.p("a.sock").to_str().unwrap(),
                "--from",
                SENDER,
                "--to",
                RECIPIENT,
                "--mail",
                mail,
                "--body",
                body,
            ])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(
            output.status.success(),
            "send failed: {stdout} {}",
            String::from_utf8_lossy(&output.stderr)
        );
        stdout
    }

    fn kill(&mut self, idx: usize) {
        self.children[idx].kill().unwrap(); // SIGKILL on Unix
        self.children[idx].wait().unwrap();
    }

    /// Restart the coordinator on the same port, replaying the same data
    /// file, so the host daemon's retry finds it again.
    fn restart_coordinator(&mut self) {
        self.kill(0);
        let bind = format!("127.0.0.1:{}", self.coord_port);
        let port_file = self.p("coord2.port");
        let start = Instant::now();
        loop {
            let _ = fs::remove_file(&port_file);
            let args: Vec<String> = vec![
                "coordinator".into(),
                "--bind".into(),
                bind.clone(),
                "--port-file".into(),
                port_file.to_str().unwrap().into(),
                "--data".into(),
                self.p("coord.log").to_str().unwrap().into(),
                "--token".into(),
                TOKEN.into(),
            ];
            let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let idx = self.spawn(&arg_refs, "coord2.out");
            let deadline = Instant::now() + Duration::from_millis(1500);
            while Instant::now() < deadline {
                if port_file.exists() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            // The port may still be held by the process we just killed.
            self.kill(idx);
            if start.elapsed() > Duration::from_secs(15) {
                panic!(
                    "coordinator never rebound {bind}: {}",
                    read(&self.p("coord2.out.err"))
                );
            }
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        for c in &mut self.children {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn read(path: &Path) -> String {
    let mut s = String::new();
    if let Ok(mut f) = fs::File::open(path) {
        let _ = f.read_to_string(&mut s);
    }
    s
}

fn wait_for_file(path: &Path, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {}", path.display());
}

fn wait_for_text(path: &Path, needle: &str, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if read(path).contains(needle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "timed out waiting for {needle:?} in {}: got {:?}",
        path.display(),
        read(path)
    );
}

fn count_lines_with(text: &str, needle: &str) -> usize {
    text.lines().filter(|l| l.contains(needle)).count()
}

#[test]
fn receipt_reaches_the_coordinator_after_it_returns() {
    let mut rig = Rig::new("receipt-late");
    rig.start_stack();
    let recipient = rig.start_recipient("recipient1.out", true);

    rig.send("m-late", "acked while the coordinator is down");
    wait_for_file(&rig.p("held.marker"), Duration::from_secs(10));

    // Coordinator down: the ack can only be committed locally.
    rig.kill(0);
    rig.kill(recipient);
    rig.start_recipient("recipient2.out", true);
    wait_for_text(
        &rig.p("daemon-b.out.err"),
        "upstream",
        Duration::from_secs(10),
    );

    let coord = read(&rig.p("coord.log"));
    assert_eq!(
        count_lines_with(&coord, "\"ev\":\"receipt\""),
        0,
        "no receipt can exist while the coordinator is down: {coord}"
    );

    rig.restart_coordinator();
    let start = Instant::now();
    loop {
        let coord = read(&rig.p("coord.log"));
        if count_lines_with(&coord, "\"ev\":\"receipt\",\"mail\":\"m-late\"") == 1 {
            assert!(coord.contains("\"record\":\"r-1\""), "{coord}");
            return;
        }
        assert!(
            count_lines_with(&coord, "\"ev\":\"receipt\",\"mail\":\"m-late\"") <= 1,
            "receipt duplicated: {coord}"
        );
        if start.elapsed() > Duration::from_secs(15) {
            panic!("locally committed receipt never reached the coordinator: {coord}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn mail_survives_recipient_kill_between_store_and_ack() {
    let mut rig = Rig::new("kill");
    rig.start_stack();
    let recipient = rig.start_recipient("recipient1.out", true);

    let body = "survive the kill [[0:FLEET:0]]";
    rig.send("m-kill", body);

    // The marker appears only after the body is fsynced to the recipient's
    // Store and before the ack is sent. Kill in that window.
    wait_for_file(&rig.p("held.marker"), Duration::from_secs(10));
    wait_for_text(
        &rig.p("recipient1.out"),
        "STORED mail=m-kill record=r-1",
        Duration::from_secs(10),
    );
    rig.kill(recipient);

    // No receipt may exist yet.
    let coord = read(&rig.p("coord.log"));
    assert_eq!(count_lines_with(&coord, "\"ev\":\"receipt\""), 0, "{coord}");

    rig.start_recipient("recipient2.out", true);
    wait_for_text(
        &rig.p("recipient2.out"),
        "DELIVERED mail=m-kill record=r-1",
        Duration::from_secs(10),
    );

    let store = read(&rig.p("recipient.store"));
    assert_eq!(
        store.lines().count(),
        1,
        "body exactly once in Store: {store}"
    );
    assert_eq!(count_lines_with(&store, body), 1, "{store}");

    let coord = read(&rig.p("coord.log"));
    assert_eq!(
        count_lines_with(&coord, "\"ev\":\"mail\",\"mail\":\"m-kill\""),
        1,
        "no duplicate row: {coord}"
    );
    assert_eq!(
        count_lines_with(&coord, "\"ev\":\"receipt\",\"mail\":\"m-kill\""),
        1,
        "{coord}"
    );
    assert!(
        coord.contains("\"record\":\"r-1\""),
        "receipt names the Store record: {coord}"
    );
}

#[test]
fn directed_mail_round_trips_once_in_order() {
    let mut rig = Rig::new("roundtrip");
    rig.start_stack();
    rig.start_recipient("recipient.out", false);

    let accepted = rig.send("m-1", "first body");
    assert!(accepted.contains("ACCEPTED mail=m-1"), "{accepted}");
    rig.send("m-2", "second [[153:Meteor Shower:7]] body");

    wait_for_text(
        &rig.p("recipient.out"),
        "DELIVERED mail=m-2",
        Duration::from_secs(10),
    );

    let store = read(&rig.p("recipient.store"));
    assert_eq!(count_lines_with(&store, "first body"), 1, "{store}");
    assert_eq!(
        count_lines_with(&store, "second [[153:Meteor Shower:7]] body"),
        1,
        "{store}"
    );
    let first = store.find("first body").unwrap();
    let second = store.find("second").unwrap();
    assert!(first < second, "order preserved: {store}");

    let coord = read(&rig.p("coord.log"));
    assert_eq!(
        count_lines_with(&coord, "\"ev\":\"mail\",\"mail\":\"m-1\""),
        1,
        "{coord}"
    );
    assert_eq!(
        count_lines_with(&coord, "\"ev\":\"receipt\",\"mail\":\"m-1\""),
        1,
        "{coord}"
    );
}
