//! Plan Step 3.3: "a stop signal drives cleanup, which today does not exist".
//! On the assembled path (`GLADE_NODE_ASSEMBLED=1`) SIGTERM or SIGINT asks the
//! node's sdax plan to shut down: a clean stop exits 0, releases the instance
//! lock and frees both ports within the witness's 2 s bound. The hand-written
//! root installs no handler, so a signal still ends it as before, by the signal
//! itself. A node killed outright (SIGKILL) cleans nothing up; one test shows
//! that its instance lock still does not outlive it (plan Step 4.4's question 3).
//!
//! Each test sets or removes the variable on the node it spawns, so it reads
//! the same whichever way the suite runs. Signals go only to the processes this
//! file spawned, by their pid, through `kill(1)`. Every file goes under a fresh
//! directory in the system temp dir, and the node runs with `GLADE_HOME` and
//! `HOME` pointed there: `~/.glade` is never touched.

// Signals are a Unix notion; the whole file is one braced conditional section.
#[cfg(unix)]
mod unix {
    use std::io::{BufRead, BufReader, Read};
    use std::net::{Ipv4Addr, TcpListener, UdpSocket};
    use std::os::unix::process::ExitStatusExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::{Duration, Instant, UNIX_EPOCH};

    const VARIABLE: &str = "GLADE_NODE_ASSEMBLED";

    /// How long a node may take to start, or to stop once signalled.
    const BOUND: Duration = Duration::from_secs(20);

    /// The witness's bound for a port to be bindable again.
    const RELEASE_BOUND: Duration = Duration::from_secs(2);

    fn scratch(test: &str) -> PathBuf {
        let nanos = UNIX_EPOCH.elapsed().unwrap().as_nanos();
        let name = format!("glade-node-{test}-{}-{nanos}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(dir.join("glade-home")).unwrap();
        dir
    }

    /// A node started from the chosen root, with its stdout lines read up to
    /// the one it was waited for (`listening`, unless the test says), if any.
    struct Node {
        child: Child,
        lines: Vec<String>,
        rest: mpsc::Receiver<String>,
    }

    impl Node {
        fn start(home: &Path, assembled: bool, args: &[&str]) -> Node {
            Node::start_until(home, assembled, args, "listening ")
        }

        fn start_until(home: &Path, assembled: bool, args: &[&str], until: &str) -> Node {
            let mut node = Node::spawn(home, assembled, args);
            let deadline = Instant::now() + BOUND;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                match node.rest.recv_timeout(left) {
                    Ok(line) => {
                        let last = line.starts_with(until);
                        node.lines.push(line);
                        if last {
                            return node;
                        }
                    }
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                        let _ = node.child.kill();
                        let _ = node.child.wait();
                        let mut stderr = String::new();
                        if let Some(mut pipe) = node.child.stderr.take() {
                            let _ = pipe.read_to_string(&mut stderr);
                        }
                        let lines = &node.lines;
                        panic!("glade-node did not print `{until}`: {lines:?}, stderr: {stderr}");
                    }
                }
            }
        }

        /// A node started from the chosen root, with no line waited for.
        fn spawn(home: &Path, assembled: bool, args: &[&str]) -> Node {
            let mut command = Command::new(env!("CARGO_BIN_EXE_glade-node"));
            command
                .args(args)
                .env("GLADE_HOME", home)
                .env("HOME", home)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if assembled {
                command.env(VARIABLE, "1");
            } else {
                command.env_remove(VARIABLE);
            }
            let mut child = command.spawn().expect("spawn glade-node");
            let stdout = child.stdout.take().unwrap();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            });
            Node {
                child,
                lines: Vec::new(),
                rest: rx,
            }
        }

        /// The rest of the first line starting `word `.
        fn value(&self, word: &str) -> String {
            let prefix = format!("{word} ");
            let found = self.lines.iter().find_map(|l| l.strip_prefix(&prefix));
            found
                .unwrap_or_else(|| panic!("no `{word}` line in {:?}", self.lines))
                .to_owned()
        }

        /// Send `signal` to this process, by its pid.
        fn signal(&self, signal: &str) {
            let pid = self.child.id().to_string();
            let status = Command::new("kill").args([signal, &pid]).status().unwrap();
            assert!(status.success(), "kill {signal} {pid}");
        }

        /// Wait for the process to end, bounded. Returns its status and stderr.
        fn wait(&mut self) -> (ExitStatus, String) {
            let deadline = Instant::now() + BOUND;
            let status = loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= deadline {
                    panic!("glade-node still ran {BOUND:?} after the signal");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            // Everything else it printed: its stdout closed when it ended.
            while let Ok(line) = self.rest.recv_timeout(BOUND) {
                self.lines.push(line);
            }
            let mut stderr = String::new();
            let mut pipe = self.child.stderr.take().unwrap();
            pipe.read_to_string(&mut stderr).unwrap();
            (status, stderr)
        }
    }

    /// A failing test never leaves its node running: whatever this file
    /// spawned and did not see end is killed, by its own handle.
    impl Drop for Node {
        fn drop(&mut self) {
            if let Ok(None) = self.child.try_wait() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }

    /// A node that must be refused as it starts: it exits 1 within the bound,
    /// and never prints `listening`. Returns its stderr.
    fn refused(home: &Path, assembled: bool, args: &[&str]) -> String {
        let mut node = Node::spawn(home, assembled, args);
        let (status, stderr) = node.wait();
        let listening = node.lines.iter().any(|l| l.starts_with("listening "));
        assert!(
            status.code() == Some(1) && !listening,
            "not refused: {status}, {:?}, stderr {stderr}",
            node.lines
        );
        stderr
    }

    /// Whether `bind` succeeds on `port` within the bound.
    fn frees(port: u16, bind: fn(u16) -> bool) -> bool {
        let started = Instant::now();
        loop {
            if bind(port) {
                return true;
            }
            if started.elapsed() >= RELEASE_BOUND {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn udp(port: u16) -> bool {
        UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
    }

    fn tcp(port: u16) -> bool {
        TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
    }

    /// The UDP port of a `peer <endpoint-id> <ip:port>` line's value.
    fn udp_port(peer: &str) -> u16 {
        peer.rsplit(':').next().unwrap().parse().unwrap()
    }

    /// Two assembled nodes, the second linked to the first. SIGTERM stops the
    /// second and SIGINT the first: each exits 0 with nothing on stderr past
    /// the root's own line, frees both ports and removes its instance lock.
    #[test]
    fn a_stop_signal_stops_the_assembled_node_cleanly() {
        let dir = scratch("stop-signal");
        let home = dir.join("glade-home");
        let b = Node::start(&home, true, &["--profile", "local", "--name", "b", "0"]);
        let target = b.value("peer").replacen(' ', "@", 1);
        let args = ["--profile", "local", "--name", "a", "--peer", &target, "0"];
        let a = Node::start(&home, true, &args);
        assert_eq!(a.value("peer-connected"), b.value("node"), "A linked to B");

        for (mut node, signal, name) in [(a, "-TERM", "a"), (b, "-INT", "b")] {
            let (udp_p, tcp_p) = (udp_port(&node.value("peer")), node.value("listening"));
            let tcp_p: u16 = tcp_p.parse().unwrap();
            let lock = home.join("sys").join(name).join("instance.lock");
            assert!(lock.exists(), "{name} holds its instance while it runs");
            node.signal(signal);
            let (status, stderr) = node.wait();
            assert_eq!(
                status.code(),
                Some(0),
                "{name} after {signal}: {status}, stderr {stderr}"
            );
            let unexpected: Vec<&str> = stderr
                .lines()
                .filter(|l| !l.starts_with("glade-node: composition root"))
                .collect();
            assert_eq!(unexpected, Vec::<&str>::new(), "{name}'s stderr");
            assert!(!lock.exists(), "{name} released its instance lock");
            assert!(
                frees(udp_p, udp),
                "{name}'s UDP port {udp_p} free within {RELEASE_BOUND:?}"
            );
            assert!(
                frees(tcp_p, tcp),
                "{name}'s TCP port {tcp_p} free within {RELEASE_BOUND:?}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A stop that arrives during start-up: the node is still dialing a peer
    /// that is gone (B, stopped first, so its endpoint id is a real one). The
    /// signal cancels the dial in flight, and the release graph still runs:
    /// exit 0, no `listening`, the dial's failure never reported (it was
    /// cancelled, not refused), the instance lock removed.
    #[test]
    fn a_stop_during_start_up_cancels_it_and_still_releases_everything() {
        let dir = scratch("stop-signal-start-up");
        let home = dir.join("glade-home");
        let mut b = Node::start(&home, true, &["--profile", "local", "--name", "b", "0"]);
        let gone = b.value("peer").replacen(' ', "@", 1);
        b.signal("-TERM");
        assert_eq!(b.wait().0.code(), Some(0), "B stopped first");

        let args = ["--profile", "local", "--name", "a", "--peer", &gone, "0"];
        let mut a = Node::start_until(&home, true, &args, "peer ");
        std::thread::sleep(Duration::from_millis(300));
        a.signal("-TERM");
        let (status, stderr) = a.wait();
        assert_eq!(status.code(), Some(0), "{status}, stderr {stderr}");
        let unexpected: Vec<&str> = stderr
            .lines()
            .filter(|l| !l.starts_with("glade-node: composition root"))
            .collect();
        assert_eq!(unexpected, Vec::<&str>::new(), "the dial was cancelled");
        assert!(
            !a.lines.iter().any(|l| l.starts_with("listening ")),
            "{:?}",
            a.lines
        );
        assert!(!home.join("sys").join("a").join("instance.lock").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The hand-written root is unchanged: it installs no handler, so SIGTERM
    /// ends it by the signal, not by an exit status.
    #[test]
    fn the_hand_written_root_still_ends_by_the_signal() {
        let dir = scratch("stop-signal-hand");
        let store = dir.join("store").display().to_string();
        let mut node = Node::start(&dir.join("glade-home"), false, &["0", &store]);
        node.signal("-TERM");
        let (status, stderr) = node.wait();
        assert_eq!(
            (status.code(), status.signal()),
            (None, Some(15)),
            "{status}: {stderr}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Plan Step 4.4's question 3 (owner, 2026-09-24): a node killed with
    /// SIGKILL cleans nothing up, so its `instance.lock` stays behind, but the
    /// OS lock on the file ends with the process. A node started on the same
    /// instance then starts; before, it was refused, "instance already
    /// locked", until someone removed the file. A second node started while
    /// that one runs is still refused. Both roots boot through
    /// `sysdir::boot_at`, and each is run. It does not prove the lock off
    /// Unix, or across hosts.
    #[test]
    fn a_node_killed_outright_restarts_and_a_second_node_is_still_refused() {
        for assembled in [false, true] {
            let dir = scratch("stop-signal-kill");
            let home = dir.join("glade-home");
            let args = ["--profile", "local", "--name", "a", "0"];
            let lock = home.join("sys").join("a").join("instance.lock");
            let mut killed = Node::start(&home, assembled, &args);
            killed.signal("-KILL");
            assert_eq!(killed.wait().0.signal(), Some(9), "killed outright");
            assert!(lock.exists(), "the killed node's lock file stays");

            let restarted = Node::start(&home, assembled, &args);
            let stderr = refused(&home, assembled, &args);
            assert!(stderr.contains("instance already locked"), "{stderr}");
            drop(restarted);
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }
}
