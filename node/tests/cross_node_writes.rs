//! Cross-node writes plan X4.3: the journey the end-to-end app relies on, as
//! one test, over loopback. Three booted nodes. H, the host (the plan's A),
//! serves the share `ws-j` (its app file's `workspace` line) and seeds B
//! `read.*,write.*` and C `read.*`; H dials both. Sessions on H and on B
//! write `value`, `log` and `crdt` ops, which fold alike on all three nodes;
//! SWMR keeps one writer across nodes; C, which may read the share but not
//! write it, is refused its write. With H stopped, B's write is
//! `UnknownShare`, and its session keeps it; once H is restarted, the write
//! sent again is `Ok`, held once, and the folds match. A session of B's,
//! acked before B knew the share, gets H's ops once H's claim reaches B.
//!
//! Each check is named for the step it pins. A failed check is noted and
//! the journey goes on, so a tree without a step shows that step's checks
//! failing (the plan: "each assertion is first seen failing on a tree
//! without the step it pins"); the test fails at its end, naming each.
//!
//! The sessions are the node's own websocket client and the wire's frames,
//! not client-rs's: a dev-dependency on client-rs would bring that crate into
//! the node gate's fmt and clippy scope. What a client keeps and sends again
//! (W5) is done here by hand.
//!
//! Signals are a Unix notion. H runs the assembled root, which answers
//! SIGTERM by releasing its links, so B learns of the stop at once; a node
//! killed outright is noticed only at its link's idle timeout. B and C run
//! whichever root the suite runs. The whole file is one braced conditional
//! section.

#[cfg(unix)]
mod unix {
    use std::io::{BufRead, BufReader, Read};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant, UNIX_EPOCH};

    use glade_node::chain::op_hash;
    use glade_node::frame::Frame;
    use glade_node::ws::{self, Msg, WsReader, WsWriter};
    use glade_wire::generated::{ErrorCode, Op, Ops, Shape, Subscribe};
    use glade_wire::swmr::{encode_swmr, SwmrAction};

    const VARIABLE: &str = "GLADE_NODE_ASSEMBLED";

    /// How long a node may take to start, to stop, or to print a line waited
    /// for.
    const BOUND: Duration = Duration::from_secs(20);

    /// How long a session waits for its next frame.
    const FRAME: Duration = Duration::from_secs(5);

    /// How long a reader goes on reading once it holds what it waits for, so
    /// that an op that should not come has had time to come.
    const QUIET: Duration = Duration::from_millis(300);

    const SHARE: &str = "ws-j";
    const VALUE: &str = "j.value";
    const LOG: &str = "j.log";
    const CRDT: &str = "j.crdt";
    const SWMR: &str = "j.swmr";

    /// A fresh directory for the test, with an empty `glade-home` inside it.
    fn scratch(test: &str) -> PathBuf {
        let nanos = UNIX_EPOCH.elapsed().unwrap().as_nanos();
        let name = format!("glade-node-{test}-{}-{nanos}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(dir.join("glade-home")).unwrap();
        dir
    }

    type Lines = Arc<Mutex<Vec<String>>>;

    /// Read `pipe` into `lines`, a line at a time, until it ends.
    fn read_into(pipe: impl Read + Send + 'static, lines: Lines) {
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines() {
                let Ok(line) = line else {
                    break;
                };
                lines.lock().unwrap().push(line);
            }
        });
    }

    /// A booted node, its stdout and stderr read for as long as it runs. One
    /// dropped while its node runs kills the node.
    struct Node {
        child: Child,
        out: Lines,
        err: Lines,
    }

    impl Node {
        /// Boot instance `--name` of `args` under `home`, on the local
        /// profile and an OS-assigned port, from the assembled root when
        /// `assembled`, else from the suite's; wait for its `listening` line.
        async fn start(home: &Path, assembled: bool, args: &[&str]) -> Node {
            let mut command = Command::new(env!("CARGO_BIN_EXE_glade-node"));
            command
                .args(["--profile", "local"])
                .args(args)
                .arg("0")
                .env("GLADE_HOME", home)
                .env("HOME", home)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if assembled {
                command.env(VARIABLE, "1");
            }
            let mut child = command.spawn().expect("spawn glade-node");
            let (out, err) = (Lines::default(), Lines::default());
            read_into(child.stdout.take().unwrap(), out.clone());
            read_into(child.stderr.take().unwrap(), err.clone());
            let node = Node { child, out, err };
            let listening = node.line(|l| l.starts_with("listening "), 1, "listening");
            listening.await.unwrap_or_else(|why| panic!("{why}"));
            node
        }

        /// The rest of the first stdout line that starts `word `.
        fn value(&self, word: &str) -> String {
            let prefix = format!("{word} ");
            let out = self.out.lock().unwrap();
            let found = out.iter().find_map(|line| line.strip_prefix(&prefix));
            let found = found.unwrap_or_else(|| panic!("no `{word}` line: {out:?}"));
            found.to_string()
        }

        /// The port the node serves clients on.
        fn port(&self) -> u16 {
            self.value("listening").parse().unwrap()
        }

        /// Where the node's endpoint listens: its `peer <tag> <ip:port>` line.
        fn peer_at(&self) -> String {
            let peer = self.value("peer");
            peer.split(' ').nth(1).unwrap().to_string()
        }

        /// Wait, bounded, until `n` of the node's stdout lines are `is`.
        async fn line(
            &self,
            is: impl Fn(&str) -> bool,
            n: usize,
            what: &str,
        ) -> Result<(), String> {
            let deadline = Instant::now() + BOUND;
            loop {
                let seen = self.out.lock().unwrap().iter().filter(|l| is(l)).count();
                if seen >= n {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    let (out, err) = (self.out.lock().unwrap(), self.err.lock().unwrap());
                    return Err(format!("no {what}: stdout {out:?}, stderr {err:?}"));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        /// Stop the node with SIGTERM, through `kill(1)`, and wait, bounded,
        /// for it to exit.
        async fn stop(mut self) -> Result<(), String> {
            let pid = self.child.id().to_string();
            let sent = Command::new("kill").args(["-TERM", &pid]).status();
            if !sent.is_ok_and(|status| status.success()) {
                return Err(format!("kill -TERM {pid} failed"));
            }
            let deadline = Instant::now() + BOUND;
            while self.child.try_wait().map_err(|e| e.to_string())?.is_none() {
                if Instant::now() >= deadline {
                    return Err(format!("the node did not stop: {:?}", self.err));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(())
        }
    }

    impl Drop for Node {
        fn drop(&mut self) {
            if let Ok(None) = self.child.try_wait() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }

    /// The endpoint id of instance `name` under `home`, as `glade-node
    /// endpoint-id` prints it, its key minted first if it has none.
    fn endpoint_id(home: &Path, name: &str) -> String {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glade-node"));
        command
            .args(["endpoint-id", "--name", name])
            .env("GLADE_HOME", home)
            .env("HOME", home)
            .env_remove(VARIABLE);
        let printed = command.output().expect("run glade-node endpoint-id");
        let said = String::from_utf8_lossy(&printed.stderr);
        assert!(printed.status.success(), "endpoint-id {name}: {said}");
        String::from_utf8(printed.stdout)
            .unwrap()
            .trim()
            .to_string()
    }

    /// `origin`'s op at `seq` on `zone` of the share, after `prev`.
    fn op(zone: &str, shape: Shape, origin: &str, seq: i64, prev: Option<&Op>) -> Op {
        Op {
            share: SHARE.into(),
            glade_id: zone.into(),
            key: vec![],
            origin: origin.into(),
            seq,
            prev: prev.map(|prev| op_hash(prev).to_vec()),
            lamport: seq,
            refs: vec![],
            shape,
            payload: format!("{origin} {zone} {seq}").into_bytes(),
        }
    }

    /// `origin`'s first op on the SWMR zone, a snapshot.
    fn snapshot(origin: &str) -> Op {
        let payload = encode_swmr(SwmrAction::Snapshot, origin.as_bytes());
        Op {
            payload,
            ..op(SWMR, Shape::Swmr, origin, 0, None)
        }
    }

    /// How a check names an op: its zone, origin and seq.
    fn named(ops: &[Op]) -> Vec<String> {
        let mut names: Vec<String> = ops
            .iter()
            .map(|op| format!("{}/{}:{}", op.glade_id, op.origin, op.seq))
            .collect();
        names.sort();
        names
    }

    /// A client session: the node's websocket client, and every op it has
    /// been sent.
    struct Session {
        r: WsReader,
        w: WsWriter,
        ops: Vec<Op>,
    }

    impl Session {
        async fn open(port: u16) -> Session {
            let (r, w) = ws::connect("127.0.0.1", port).await.unwrap();
            let ops = Vec::new();
            Session { r, w, ops }
        }

        async fn send(&self, frame: Frame) -> Result<(), String> {
            let sent = self.w.send_binary(&frame.to_bytes()).await;
            sent.map_err(|e| format!("send: {e}"))
        }

        /// The next frame within `wait`, or `None`; an op is kept too.
        async fn within(&mut self, wait: Duration) -> Result<Option<Frame>, String> {
            let Ok(read) = tokio::time::timeout(wait, self.r.read()).await else {
                return Ok(None);
            };
            let Msg::Binary(bytes) = read.map_err(|e| format!("read: {e}"))? else {
                return Err("the node closed the session".into());
            };
            let frame = Frame::from_bytes(&bytes).map_err(|e| format!("decode: {e:?}"))?;
            if let Frame::Ops(ops) = &frame {
                self.ops.extend(ops.ops.iter().cloned());
            }
            Ok(Some(frame))
        }

        /// The next frame, bounded: none is a failure, naming `what`.
        async fn next(&mut self, what: &str) -> Result<Frame, String> {
            let next = self.within(FRAME).await?;
            next.ok_or_else(|| format!("no frame in {FRAME:?} waiting for {what}"))
        }

        /// Subscribe each of `zones` of the share and read its ack; a refused
        /// subscribe (R6) is a failure, naming its reason.
        async fn subscribe(&mut self, zones: &[&str]) -> Result<(), String> {
            for zone in zones {
                let (share, glade_id) = (SHARE.to_string(), zone.to_string());
                let (key, from) = (None, None);
                let subscribe = Subscribe {
                    share,
                    glade_id,
                    key,
                    from,
                };
                self.send(Frame::Subscribe(subscribe)).await?;
                loop {
                    match self.next("an ack").await? {
                        Frame::Heads(h) if h.streams.is_empty() => {
                            let reason = self.next("the refusal's reason").await?;
                            return Err(format!("{zone} refused: {reason:?}"));
                        }
                        Frame::Heads(_) => break,
                        _ => {}
                    }
                }
            }
            Ok(())
        }

        /// Write `op` and read its status (R1): its code and its message.
        async fn write(&mut self, op: &Op) -> Result<(ErrorCode, String), String> {
            let ops = vec![op.clone()];
            self.send(Frame::Ops(Ops { ops, pri: None })).await?;
            let hash = op_hash(op);
            let corr: String = hash.iter().map(|b| format!("{b:02x}")).collect();
            loop {
                if let Frame::Error(e) = self.next("the op's status").await? {
                    if e.corr.as_deref() == Some(corr.as_str()) {
                        return Ok((e.code, e.message));
                    }
                }
            }
        }

        /// Read until the session has been sent every op of `want`, then for
        /// [`QUIET`] more: it must have been sent those ops, each once, and
        /// no other.
        async fn holds(&mut self, want: &[&Op]) -> Result<(), String> {
            let want: Vec<Op> = want.iter().map(|&op| op.clone()).collect();
            while !want.iter().all(|op| self.ops.contains(op)) {
                let held = named(&self.ops);
                let waiting = |e: String| format!("{e}; held {held:?}");
                self.next("the ops to fold").await.map_err(waiting)?;
            }
            let quiet = Instant::now() + QUIET;
            while let Some(left) = quiet.checked_duration_since(Instant::now()) {
                if self.within(left).await?.is_none() {
                    break;
                }
            }
            let (held, wanted) = (named(&self.ops), named(&want));
            if held != wanted {
                return Err(format!("held {held:?}, not {wanted:?}"));
            }
            Ok(())
        }
    }

    /// Whether a status is `code` with a message that holds `says`.
    fn is(
        status: Result<(ErrorCode, String), String>,
        code: ErrorCode,
        says: &str,
    ) -> Result<(), String> {
        let (got, message) = status?;
        if got != code || !message.contains(says) {
            return Err(format!(
                "{got:?} \"{message}\", not {code:?} with \"{says}\""
            ));
        }
        Ok(())
    }

    /// The journey's checks, each named for the step it pins: one that fails
    /// is noted, and the journey goes on.
    #[derive(Default)]
    struct Checks(Vec<String>);

    impl Checks {
        fn check(&mut self, name: &str, outcome: Result<(), String>) {
            match outcome {
                Ok(()) => eprintln!("ok   {name}"),
                Err(why) => {
                    eprintln!("FAIL {name}: {why}");
                    self.0.push(format!("{name}: {why}"));
                }
            }
        }
    }

    /// Readers on each of `nodes` (a name and its port), subscribed to
    /// `zones`, hold exactly `want`: the folds are alike. `check` names it.
    async fn fold_alike(
        checks: &mut Checks,
        check: &str,
        nodes: [(&str, u16); 3],
        zones: &[&str],
        want: &[&Op],
    ) {
        for (name, port) in nodes {
            let mut reader = Session::open(port).await;
            let held = match reader.subscribe(zones).await {
                Ok(()) => reader.holds(want).await,
                Err(why) => Err(why),
            };
            checks.check(&format!("{check}, at {name}"), held);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn writes_from_two_nodes_converge_through_a_restart_of_the_holder() {
        let dir = scratch("cross-node-writes");
        let home = dir.join("glade-home");
        let mut checks = Checks::default();
        let keys = ["h", "b", "c"].map(|name| endpoint_id(&home, name));
        let [h_key, b_key, c_key] = &keys;

        // B and C admit H's endpoint, and dial no one.
        let b = Node::start(&home, false, &["--name", "b", "--peer", h_key]).await;
        let c = Node::start(&home, false, &["--name", "c", "--peer", h_key]).await;
        let (b_id, c_id) = (b.value("node"), c.value("node"));

        // A session of B's, acked from B's replica before B knows the share.
        let mut early = Session::open(b.port()).await;
        early.subscribe(&[VALUE]).await.unwrap();

        // H serves the share, lets B read and write it and C read it, and
        // dials both.
        let app = dir.join("journey.glade");
        let seeds = format!("seed {b_id} {SHARE} read.*,write.*\nseed {c_id} {SHARE} read.*\n");
        let text = format!("glade-app v1\napp journey\n{seeds}workspace {SHARE} journey\n");
        std::fs::write(&app, text).unwrap();
        let app = app.display().to_string();
        let (to_b, to_c) = (
            format!("{b_key}@{}", b.peer_at()),
            format!("{c_key}@{}", c.peer_at()),
        );
        let h_args = [
            "--name", "h", "--app", &app, "--peer", &to_b, "--peer", &to_c,
        ];
        let h = Node::start(&home, true, &h_args).await;
        let h_id = h.value("node");
        let round = format!("home round with node {h_id}:");
        for node in [&b, &c] {
            let pulled = node.line(|l| l.starts_with(&round), 1, "a home round with H");
            pulled.await.unwrap();
        }

        // h, a session of H's, writes each shape there.
        let shapes = [
            (VALUE, Shape::Value),
            (LOG, Shape::Log),
            (CRDT, Shape::Crdt),
        ];
        let mut hw = Session::open(h.port()).await;
        let by_h = shapes.map(|(zone, shape)| op(zone, shape, "h-writer", 0, None));
        for op in &by_h {
            is(hw.write(op).await, ErrorCode::Ok, "").unwrap();
        }

        let x42 = "J1 X4.2b: B's session, acked before H's claim landed, gets H's op unasked";
        checks.check(x42, early.holds(&[&by_h[0]]).await);

        // b, a session of B's, writes each shape there: each crosses to H.
        let mut bw = Session::open(b.port()).await;
        let by_b = shapes.map(|(zone, shape)| op(zone, shape, "b-writer", 0, None));
        let mut statuses = Ok(());
        for op in &by_b {
            statuses = statuses.and(is(bw.write(op).await, ErrorCode::Ok, ""));
        }
        let x32b = "J2 X3.2b: b's writes at B are Ok, as H answered them";
        checks.check(x32b, statuses);
        let nodes = [("H", h.port()), ("B", b.port()), ("C", c.port())];
        let both: Vec<&Op> = by_h.iter().chain(&by_b).collect();
        let zones = [VALUE, LOG, CRDT];
        let x32 = "J3 X3.2: the writes from both sides fold alike on all three";
        fold_alike(&mut checks, x32, nodes, &zones, &both).await;

        // SWMR's one writer is H's to decide, across nodes (W7).
        let (first, second) = (snapshot("h-writer"), snapshot("b-writer"));
        is(hw.write(&first).await, ErrorCode::Ok, "").unwrap();
        let conflict = is(
            bw.write(&second).await,
            ErrorCode::Protocol,
            "SWMR writer conflict",
        );
        checks.check("J4 X3.2: a second SWMR writer, at B, is refused", conflict);

        // C may read the share, but holds no grant to write it.
        let mut cw = Session::open(c.port()).await;
        let by_c = op(VALUE, Shape::Value, "c-writer", 0, None);
        let why = format!("unauthorized: node {c_id} holds no grant of write.append on {SHARE}");
        let refused = is(cw.write(&by_c).await, ErrorCode::Unauthorized, &why);
        checks.check("J5 X4.1: C's write is refused by its node id", refused);

        // H stops: b's next write is not placed, and b keeps it (W5).
        h.stop().await.unwrap();
        let closed = format!("link {h_id} closed");
        b.line(|l| l == closed, 1, "B's link to H closed")
            .await
            .unwrap();
        let kept = op(LOG, Shape::Log, "b-writer", 1, Some(&by_b[1]));
        let unplaced = is(bw.write(&kept).await, ErrorCode::UnknownShare, "");
        checks.check(
            "J6 X2.3: with H stopped, b's write is UnknownShare",
            unplaced,
        );

        // H restarts and dials again. b subscribes the zone, then sends the
        // write it kept again (W5): `Ok`, once, and the folds match.
        let h = Node::start(&home, true, &h_args).await;
        for node in [&b, &c] {
            let pulled = node.line(|l| l.starts_with(&round), 2, "a home round with H again");
            pulled.await.unwrap();
        }
        bw.subscribe(&[LOG]).await.unwrap();
        let resent = is(bw.write(&kept).await, ErrorCode::Ok, "");
        let x32b = "J7 X3.2b: b's kept write, sent again, is Ok, as H answered it";
        checks.check(x32b, resent);
        let nodes = [("H", h.port()), ("B", b.port()), ("C", c.port())];
        let all: Vec<&Op> = both.into_iter().chain([&first, &kept]).collect();
        let zones = [VALUE, LOG, CRDT, SWMR];
        let x32 = "J8 X3.2: after the restart, the folds match on all three, each op once";
        fold_alike(&mut checks, x32, nodes, &zones, &all).await;

        drop((h, b, c));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(checks.0.is_empty(), "failed:\n{}", checks.0.join("\n"));
    }
}
