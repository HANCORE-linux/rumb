use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

const DEMO_STREAM: &str = include_str!("../demo.ndjson");

/// One line of `rumb-scan --stream`.
#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Event {
    Start {
        last_upgrade: Option<String>,
        problems_since: String,
        #[serde(default)]
        now: Option<String>,
    },
    Step {
        stage: String,
        label: String,
    },
    Stage {
        stage: String,
        data: Value,
    },
    Done {
        label: String,
        #[serde(default)]
        value: Value,
    },
    Result {},
    /// A line from the package manager's log while it changes the system,
    /// e.g. "upgraded hyprland (0.55.2-1 -> 0.56.0-1)".
    Activity {
        line: String,
    },
    Journal {
        time: String,
        ident: String,
        message: String,
    },
}

pub enum Msg {
    Event(Event, String),
    Invalid,
    Eof { stderr: String },
}

/// Finds the scan script: $RUMB_SCAN, then `bin/` next to this binary or one
/// of its parents (the repo layout), then $PATH.
pub fn find_script() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("RUMB_SCAN") {
        return Some(PathBuf::from(path));
    }

    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors().skip(1).take(5) {
            let candidate = dir.join("bin/rumb-scan");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("rumb-scan"))
            .find(|candidate| candidate.is_file())
    })
}

static GROUP: AtomicI32 = AtomicI32::new(0);

pub fn terminate_scan() {
    let group = GROUP.load(Ordering::Relaxed);
    if group != 0 {
        unsafe { libc::kill(-group, libc::SIGTERM) };
    }
}

pub struct Scan {
    child: Child,
}

impl Scan {
    pub fn exited(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}

impl Drop for Scan {
    fn drop(&mut self) {
        let _ = GROUP.compare_exchange(self.child.id() as i32, 0, Ordering::Relaxed, Ordering::Relaxed);
        if self.exited().is_some() {
            return;
        }
        let group = -(self.child.id() as libc::pid_t);
        unsafe { libc::kill(group, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if self.exited().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        unsafe { libc::kill(group, libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

pub fn spawn_script(script: &Path, args: &[String]) -> io::Result<(Scan, Receiver<Msg>)> {
    let mut child = Command::new(script)
        .arg("--stream")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    GROUP.store(child.id() as i32, Ordering::Relaxed);

    let stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let (tx, rx) = mpsc::channel();

    // stderr gets its own reader so a chatty script can never block on a full pipe
    let stderr_reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let msg = match serde_json::from_str::<Event>(&line) {
                Ok(event) => Msg::Event(event, line),
                Err(_) => Msg::Invalid,
            };
            if tx.send(msg).is_err() {
                return;
            }
        }
        let stderr = stderr_reader.join().unwrap_or_default();
        let _ = tx.send(Msg::Eof { stderr });
    });

    Ok((Scan { child }, rx))
}

/// Replays the bundled demo stream; each line may carry a `_delay` in ms.
pub fn spawn_demo() -> Receiver<Msg> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in DEMO_STREAM.lines().filter(|line| !line.trim().is_empty()) {
            let delay = serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|value| value.get("_delay").and_then(Value::as_u64))
                .unwrap_or(0);
            thread::sleep(Duration::from_millis(delay));
            let msg = match serde_json::from_str::<Event>(line) {
                Ok(event) => Msg::Event(event, line.to_string()),
                Err(_) => Msg::Invalid,
            };
            if tx.send(msg).is_err() {
                return;
            }
        }
        let _ = tx.send(Msg::Eof { stderr: String::new() });
    });
    rx
}
