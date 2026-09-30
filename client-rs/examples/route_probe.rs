//! The route probe (plan Step 4.6 part 4; `dev-docs/GladeFixedPeerRoute.md`
//! section 2): one long-lived client session, which the route journey drives a
//! line at a time. It is built on `glade-client` alone, with no node internals
//! (P00-a).
//!
//! `route_probe <url> <principal>` connects to `<url>` (`ws://<host>:<port>`),
//! says `hello` with `<principal>`, which is also its origin, and prints
//! `welcome <principal>`. If it cannot, it prints `error connect: <reason>` and
//! exits 1. It then reads one command per line on stdin and prints one line
//! per answer, in the order of the commands, and one line per event:
//!
//! ```text
//! subscribe <zone>                 acked <zone> [<op> ...]   the heads, once the replay is in
//!                                  refused <zone> <code>: <message>
//! append <zone> <shape> <payload>  <outcome> <zone> <op> <payload> ...   the node's answer
//! resend-last                      the same, for the last op sent, sent again byte for byte
//! log <zone>                       log <zone> [<payload> ...]   the log this session folds
//! reconnect                        welcome <principal>   a new connection to <url>, and hello
//! quit                             bye, and it exits 0, as it does at the end of stdin
//! anything that cannot run         error <command>: <reason>
//!
//! event: an op arrives             op <zone> <op> <payload>
//! event: a zone refused after ack  zone-refused <zone> <code>: <message>
//! event: the connection drops      dropped
//! ```
//!
//! An op's outcome is `ok` (the node holds it), `retained`, `unknown` (the
//! connection ended first), `not-placed ...: <message>` or
//! `refused ... <code>: <message>`. A zone is `<share>/<glade_id>`, the
//! commons (a keyed zone's key would follow as `/<key>`); an op is
//! `<origin>:<seq>`; a code is the wire's name for it, or `-` if none came. A
//! payload or name is printed as itself when it is a word (ASCII letters,
//! digits, `.`, `_` and `-`, not starting `0x`), and otherwise as `0x` and its
//! bytes in hex; a command's payload is read the same way. Events that arrive
//! while a command runs are printed after its answer.

use std::io::{self, BufRead, Write};

use glade_client::{GladeClient, OpOutcome, SubscribeOutcome};
use glade_wire::generated::{ErrorCode, Op};
use tokio::sync::mpsc;

const USAGE: &str = "a command is subscribe <zone>, append <zone> <shape> <payload>, \
                     resend-last, log <zone>, reconnect or quit";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [url, principal] = args.as_slice() else {
        eprintln!("usage: route_probe <url> <principal>");
        std::process::exit(2);
    };
    if let Err(e) = run(url, principal).await {
        eprintln!("route_probe: {e}");
        std::process::exit(1);
    }
}

/// Connect and say hello, then answer each command and print each event,
/// until `quit` or the end of stdin.
async fn run(url: &str, principal: &str) -> io::Result<()> {
    let client = GladeClient::new(principal);
    let mut ops = client.on_ops().await;
    let mut refusals = client.on_zone_refused().await;
    let mut drops = client.on_drop().await;
    let connected = async {
        client.connect(url).await?;
        client.hello(Some(principal)).await
    };
    if let Err(e) = connected.await {
        say(&format!("error connect: {}", one_line(&e.to_string())))?;
        return Err(e);
    }
    say(&welcome(principal))?;
    let mut commands = stdin_lines();
    let mut last = None;
    loop {
        tokio::select! {
            biased;
            Some(arrived) = ops.recv() => {
                for op in &arrived {
                    say(&format!("op {}", named(op)))?;
                }
            }
            Some(refusal) = refusals.recv() => {
                let zone = zone(&refusal.share, &refusal.glade_id, &refusal.key);
                let why = reason(Some(refusal.code), &refusal.message);
                say(&format!("zone-refused {zone} {why}"))?;
            }
            Some(()) = drops.recv() => say("dropped")?,
            line = commands.recv() => {
                let Some(line) = line else {
                    break;
                };
                let Some(reply) = answer(&client, principal, &mut last, &line).await else {
                    client.close().await;
                    return say("bye");
                };
                say(&reply)?;
            }
        }
    }
    client.close().await;
    Ok(())
}

/// The answer to one command line, or `None` for `quit`.
async fn answer(
    client: &GladeClient,
    principal: &str,
    last: &mut Option<Op>,
    line: &str,
) -> Option<String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let answered = match words.as_slice() {
        ["subscribe", at] => subscribe(client, at).await,
        ["append", at, shape, payload] => append(client, last, at, shape, payload).await,
        ["resend-last"] => resend(client, last.as_ref()).await,
        ["log", at] => log(client, at).await,
        ["reconnect"] => reconnect(client, principal).await,
        ["quit"] => return None,
        _ => Err(invalid(USAGE)),
    };
    let command = words.first().copied().unwrap_or_default();
    Some(answered.unwrap_or_else(|e| format!("error {command}: {}", one_line(&e.to_string()))))
}

async fn subscribe(client: &GladeClient, text: &str) -> io::Result<String> {
    let (share, glade_id) = parse_zone(text)?;
    let shown = zone(share, glade_id, &[]);
    let answer = match client.subscribe_outcome(share, glade_id, None).await? {
        SubscribeOutcome::Accepted { heads } => {
            let heads: Vec<String> = heads
                .iter()
                .map(|head| format!("{}:{}", word(head.origin.as_bytes()), head.seq))
                .collect();
            format!("acked {shown} [{}]", heads.join(" "))
        }
        SubscribeOutcome::Refused { code, message } => {
            format!("refused {shown} {}", reason(code, &message))
        }
    };
    Ok(answer)
}

async fn append(
    client: &GladeClient,
    last: &mut Option<Op>,
    text: &str,
    shape: &str,
    payload: &str,
) -> io::Result<String> {
    let (share, glade_id) = parse_zone(text)?;
    let payload = payload_bytes(payload)?;
    let appended = client.append_outcome(share, glade_id, shape, payload, None);
    let (op, outcome) = appended.await?;
    let answer = outcome_line(&op, outcome);
    *last = Some(op);
    Ok(answer)
}

/// The last op sent, sent again byte for byte, and the node's answer to it.
async fn resend(client: &GladeClient, last: Option<&Op>) -> io::Result<String> {
    let op = last.ok_or_else(|| invalid("no op sent yet"))?;
    let outcomes = client.send_ops_outcome(vec![op.clone()]).await?;
    let outcome = outcomes.into_iter().next().unwrap_or(OpOutcome::Unknown);
    Ok(outcome_line(op, outcome))
}

async fn log(client: &GladeClient, text: &str) -> io::Result<String> {
    let (share, glade_id) = parse_zone(text)?;
    let entries = client.fold_log(share, glade_id, None).await;
    let entries: Vec<String> = entries.iter().map(|entry| word(entry)).collect();
    let zone = zone(share, glade_id, &[]);
    Ok(format!("log {zone} [{}]", entries.join(" ")))
}

/// A new connection to the same node, and hello again: no subscription
/// outlives its connection, and the session's chains go on.
async fn reconnect(client: &GladeClient, principal: &str) -> io::Result<String> {
    client.reconnect().await?;
    client.hello(Some(principal)).await?;
    Ok(welcome(principal))
}

fn welcome(principal: &str) -> String {
    format!("welcome {}", word(principal.as_bytes()))
}

/// An op's answer: its outcome, then the op.
fn outcome_line(op: &Op, outcome: OpOutcome) -> String {
    let op = named(op);
    match outcome {
        OpOutcome::Accepted => format!("ok {op}"),
        OpOutcome::Retained => format!("retained {op}"),
        OpOutcome::NotPlaced { message } => format!("not-placed {op}: {}", one_line(&message)),
        OpOutcome::Refused { code, message } => {
            format!("refused {op} {}", reason(Some(code), &message))
        }
        OpOutcome::Unknown => format!("unknown {op}"),
    }
}

/// `<zone> <origin>:<seq> <payload>`.
fn named(op: &Op) -> String {
    let zone = zone(&op.share, &op.glade_id, &op.key);
    let origin = word(op.origin.as_bytes());
    format!("{zone} {origin}:{} {}", op.seq, word(&op.payload))
}

fn zone(share: &str, glade_id: &str, key: &[u8]) -> String {
    let zone = format!("{}/{}", word(share.as_bytes()), word(glade_id.as_bytes()));
    if key.is_empty() {
        zone
    } else {
        format!("{zone}/{}", word(key))
    }
}

/// A command's `<share>/<glade_id>`.
fn parse_zone(text: &str) -> io::Result<(&str, &str)> {
    match text.split_once('/') {
        Some((share, glade_id)) if !share.is_empty() && !glade_id.is_empty() => {
            Ok((share, glade_id))
        }
        _ => Err(invalid("a zone is <share>/<glade_id>")),
    }
}

/// `<code>: <message>`.
fn reason(code: Option<ErrorCode>, message: &str) -> String {
    let code = code.map_or_else(|| "-".to_string(), |code| format!("{code:?}"));
    format!("{code}: {}", one_line(message))
}

/// A message may not end the line it is printed on.
fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

/// `bytes` as themselves when they are a word, else as `0x` and their hex.
fn word(bytes: &[u8]) -> String {
    let plain = |b: &u8| b.is_ascii_alphanumeric() || b"._-".contains(b);
    if !bytes.is_empty() && !bytes.starts_with(b"0x") && bytes.iter().all(plain) {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("0x{}", hex.concat())
}

/// A command's payload: `0x` and an even number of hex digits, else the
/// token's own bytes.
fn payload_bytes(token: &str) -> io::Result<Vec<u8>> {
    let Some(hex) = token.strip_prefix("0x") else {
        return Ok(token.as_bytes().to_vec());
    };
    let digit = |b: &u8| char::from(*b).to_digit(16);
    let pairs = hex.as_bytes().chunks(2);
    let bytes: Option<Vec<u8>> = pairs
        .map(|pair| match pair {
            [high, low] => u8::try_from(digit(high)? * 16 + digit(low)?).ok(),
            _ => None,
        })
        .collect();
    bytes.ok_or_else(|| invalid("a 0x payload is an even number of hex digits"))
}

fn invalid(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, why)
}

/// Print one line, and flush it: the journey reads each as it comes.
fn say(line: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "{line}")?;
    out.flush()
}

/// Stdin's lines as they come, read on a thread of their own; the channel
/// closes at the end of stdin.
fn stdin_lines() -> mpsc::UnboundedReceiver<String> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}
