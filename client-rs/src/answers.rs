//! The node's answers to this client's ops and subscribes (GladeSubstrateV1 §6,
//! R1 and R5 to R7). Each op the client sends is kept by its hash until a
//! status names it by `corr`, never by its place among other frames. `Ok` and
//! `Retention` settle it; `UnknownShare` leaves it not placed, kept to be sent
//! again, zone by zone (W5); any other code refuses it, and the session drops it
//! with the rest of its chain (answer 4). No op goes past an op of its chain not
//! placed, and a gap refusal past one is not placed either (F6). A subscribe
//! waits for its ack, which names its zone and each origin's head there, or
//! names none for a refusal whose reason follows (R6); then for its replay,
//! which is in once the connection has received, or sent and had answered
//! `Ok`, an op at or above each head (R7). The zone an ack names is live until
//! the connection ends, or an `Error` naming no op, and no refused subscribe's
//! reason, refuses it after its ack (F13). That `Error` names a share and
//! stream, not a key, so it refuses each live zone of them. Pure: no socket
//! and no clock (LBT-008). `client.rs` feeds it the frames and tells the
//! waiters it hands back.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use glade_wire::generated::{Error, ErrorCode, Head, Heads, Op};

use crate::hash::op_hash;
use crate::session::Session;

/// How many sent ops wait for a status at once. A node from before Phase 2 of
/// the client-writes plan answers none, so past this the oldest is let go,
/// its fate unknown.
pub const WAITING_BOUND: usize = 4096;

/// A zone, `(share, glade_id, key)`: W5 paces its resends zone by zone.
pub type Zone = (String, String, Vec<u8>);

pub fn zone_of(op: &Op) -> Zone {
    (op.share.clone(), op.glade_id.clone(), op.key.clone())
}

/// The node's answer to one op this client sent (R1, R7).
#[derive(Clone, Debug, PartialEq)]
pub enum OpOutcome {
    /// `Ok`: the node holds the op, appended now or held byte for byte (R2).
    Accepted,
    /// `Retention`: below the first seq the node holds on its chain. Settled,
    /// not refused: nothing is dropped, and the chain goes on.
    Retained,
    /// `UnknownShare`: not placed (W5). The client keeps the op and its chain,
    /// and sends them again. So too a `Protocol` gap past an op of its chain
    /// not placed, and an op held back behind one, which the node never saw
    /// (F6).
    NotPlaced { message: String },
    /// Any other code. An op the session made is dropped with the later ops
    /// of its chain, which waits for a subscribe of its zone (answer 4).
    Refused { code: ErrorCode, message: String },
    /// No status: the connection ended first, or none came (R1).
    Unknown,
}

/// An op this client sent, with the node's status for it, as `on_refused` and
/// `on_unplaced` report it.
#[derive(Clone, Debug, PartialEq)]
pub struct OpStatus {
    pub op: Op,
    pub code: ErrorCode,
    pub message: String,
}

/// A sent op, answered: what became of it, and who waits to hear.
#[derive(Debug, PartialEq)]
pub struct Answered<W> {
    pub op: Op,
    pub outcome: OpOutcome,
    pub waiter: Option<W>,
    /// Not placed for the first time, so `on_unplaced` hears of it, once.
    pub newly_unplaced: bool,
}

struct Sent<W> {
    hash: String,
    op: Op,
    waiter: Option<W>,
}

/// The sends not yet answered, oldest first; and (W5) the ops not placed, in
/// the order they were found so, held ones (F6) with them, with each zone's
/// resend count and timer.
pub struct Answers<W> {
    waiting: VecDeque<Sent<W>>,
    bound: usize,
    unplaced: Vec<Op>,
    /// Each zone's resends since it last had every op placed.
    resends: HashMap<Zone, u32>,
    /// The zones whose resend timer runs, by the timer's number.
    timers: HashMap<Zone, u64>,
    timer_count: u64,
}

impl<W> Answers<W> {
    pub fn new(bound: usize) -> Self {
        Answers { waiting: VecDeque::new(), bound, unplaced: Vec::new(), resends: HashMap::new(), timers: HashMap::new(), timer_count: 0 }
    }

    /// How many sends wait for a status.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// Of ops to send, in order, those to put on the wire now, each kept by
    /// its hash until its status comes (R1); and the answers due at once:
    /// `Unknown` to a send let go past the bound, `NotPlaced` to an op held.
    /// F6: no op goes past an op of its chain not placed and not yet sent
    /// again. It is held with that op, not placed, and goes after it, in
    /// order, when that op is sent again (`unplaced_in`).
    pub fn to_send(
        &mut self,
        ops: Vec<Op>,
        waiters: Vec<Option<W>>,
    ) -> (Vec<Op>, Vec<Answered<W>>) {
        let mut now = Vec::new();
        let mut answered = Vec::new();
        for (op, waiter) in ops.into_iter().zip(waiters) {
            let before = self.unplaced_before(&op).find(|kept| !self.in_flight(kept));
            if let Some(seq) = before.map(|kept| kept.seq) {
                let newly_unplaced = !self.unplaced.contains(&op);
                if newly_unplaced {
                    self.unplaced.push(op.clone());
                }
                let message = format!("not sent: seq {seq} of its chain is not placed");
                let outcome = OpOutcome::NotPlaced { message };
                answered.push(Answered {
                    op,
                    outcome,
                    waiter,
                    newly_unplaced,
                });
                continue;
            }
            answered.extend(self.sent(op.clone(), waiter));
            now.push(op);
        }
        (now, answered)
    }

    /// Keep a sent op until its status comes. Past the bound the oldest send
    /// is let go, answered `Unknown`.
    pub fn sent(&mut self, op: Op, waiter: Option<W>) -> Option<Answered<W>> {
        let hash = op_hash(&op).iter().map(|b| format!("{b:02x}")).collect();
        self.waiting.push_back(Sent { hash, op, waiter });
        if self.waiting.len() <= self.bound {
            return None;
        }
        self.waiting.pop_front().map(unknown)
    }

    /// Apply one status. It answers the oldest send of the op its `corr`
    /// names, so an op sent twice takes its two statuses in order. A status
    /// naming nothing this client waits on, or no op at all (R6's reason),
    /// changes nothing.
    pub fn status(&mut self, session: &mut Session, status: &Error) -> Option<Answered<W>> {
        let corr = status.corr.as_deref()?;
        let at = self.waiting.iter().position(|sent| sent.hash == corr)?;
        let Sent { op, waiter, .. } = self.waiting.remove(at)?;
        let message = status.message.clone();
        let outcome = match status.code {
            ErrorCode::Ok => OpOutcome::Accepted,
            ErrorCode::Retention => OpOutcome::Retained,
            ErrorCode::UnknownShare => OpOutcome::NotPlaced { message },
            // F6: a gap past an op of its chain not placed is not placed
            // either. The client knows it by its place: any other `Protocol`
            // comes again when the ops go again, in order, and is refused then.
            ErrorCode::Protocol if self.unplaced_before(&op).next().is_some() => {
                OpOutcome::NotPlaced { message }
            }
            code => OpOutcome::Refused { code, message },
        };
        let mut newly_unplaced = false;
        match &outcome {
            OpOutcome::NotPlaced { .. } => {
                if !self.unplaced.contains(&op) {
                    self.unplaced.push(op.clone());
                    newly_unplaced = true;
                }
            }
            OpOutcome::Refused { .. } => {
                // A refusal still drops the tail, the kept ops of W5 with it.
                self.unplaced.retain(|kept| !(same_chain(kept, &op) && kept.seq >= op.seq));
                session.refused(&op);
            }
            _ => {
                self.unplaced.retain(|kept| *kept != op);
            }
        }
        let zone = zone_of(&op);
        if !self.unplaced.iter().any(|kept| in_zone(kept, &zone)) {
            // Every op of the zone is placed: its timer stops, its count restarts.
            self.resends.remove(&zone);
            self.timers.remove(&zone);
        }
        Some(Answered { op, outcome, waiter, newly_unplaced })
    }

    /// The connection ended, so no status is coming for what was sent on it
    /// (R7): every send is answered `Unknown`. Ops not placed stay, and no
    /// resend timer runs until an ack on a new connection sends them (W5).
    pub fn ended(&mut self) -> Vec<Answered<W>> {
        self.timers.clear();
        self.waiting.drain(..).map(unknown).collect()
    }

    /// W5: a zone's ops not placed, in their chains' order, to send again.
    /// One already sent again, and waiting for its status, is left out.
    pub fn unplaced_in(&self, zone: &Zone) -> Vec<Op> {
        let mut ops: Vec<Op> = self
            .unplaced
            .iter()
            .filter(|op| in_zone(op, zone) && !self.in_flight(op))
            .cloned()
            .collect();
        ops.sort_by(|a, b| a.origin.cmp(&b.origin).then(a.seq.cmp(&b.seq)));
        ops
    }

    /// W5: a resend timer to start for the zone, by its number, when the zone
    /// has ops not placed and none runs.
    pub fn start_resending(&mut self, zone: &Zone) -> Option<u64> {
        if self.timers.contains_key(zone) || !self.unplaced.iter().any(|op| in_zone(op, zone)) {
            return None;
        }
        self.timer_count += 1;
        self.timers.insert(zone.clone(), self.timer_count);
        Some(self.timer_count)
    }

    /// W5: the wait before the zone's next resend, which it counts.
    pub fn next_resend(&mut self, zone: &Zone) -> Duration {
        let count = self.resends.entry(zone.clone()).or_insert(0);
        let wait = resend_delay(*count);
        *count = count.saturating_add(1);
        wait
    }

    /// W5: what the zone's timer `timer` sends again on this tick, or `None`
    /// when it has stopped.
    pub fn resend_tick(&mut self, zone: &Zone, timer: u64) -> Option<Vec<Op>> {
        if self.timers.get(zone) != Some(&timer) {
            return None;
        }
        Some(self.unplaced_in(zone))
    }

    /// F6: the ops of `op`'s chain before it that are not placed.
    fn unplaced_before<'a>(&'a self, op: &'a Op) -> impl Iterator<Item = &'a Op> {
        let before = move |kept: &&Op| same_chain(kept, op) && kept.seq < op.seq;
        self.unplaced.iter().filter(before)
    }

    /// Sent, and waiting for its status.
    fn in_flight(&self, op: &Op) -> bool {
        self.waiting.iter().any(|sent| sent.op == *op)
    }
}

/// A subscribe's answer, as data (R5, R6).
#[derive(Clone, Debug, PartialEq)]
pub enum SubscribeOutcome {
    /// Taken, and its replay is in: each origin's head in the zone at the ack,
    /// by seq and hash (R5, R7).
    Accepted { heads: Vec<Head> },
    /// Refused (R6), with the reason's code and message. No code when the
    /// reason did not come before the connection ended.
    Refused { code: Option<ErrorCode>, message: String },
}

/// A subscribe answered: its zone, what became of it, and who waits to hear.
#[derive(Debug, PartialEq)]
pub struct Subscribed<S> {
    pub zone: Zone,
    pub outcome: SubscribeOutcome,
    pub waiter: S,
}

/// Every waiting subscribe, once none can be matched to its answer: the
/// connection ended, or an ack came for another zone in a subscribe's turn.
/// One already refused stays a refusal, its reason unknown; the rest fail.
#[derive(Debug, PartialEq)]
pub struct Abandoned<S> {
    pub failed: Vec<S>,
    pub refused: Vec<Subscribed<S>>,
}

/// A zone refused after its subscribe was acked (F13), as `on_zone_refused`
/// reports it: the node ended this connection's subscription with an `Error`
/// naming no op, as a forwarding node relaying its claim holder's refusal of
/// the read does (F5), and the grant re-check pass. The wire names the share
/// and stream only; the key is the zone's, as the client subscribed it.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneRefusal {
    pub share: String,
    pub glade_id: String,
    pub key: Vec<u8>,
    pub code: ErrorCode,
    pub message: String,
}

/// What one `Error` naming no op refused after the ack (F13): the live zones
/// it names, and the subscribes of them still waiting for their replay.
#[derive(Debug, PartialEq)]
pub struct RefusedAfterAck<S> {
    pub zones: Vec<ZoneRefusal>,
    pub subscribes: Vec<Subscribed<S>>,
}

/// The subscribes waiting on the node, the highest seq of each origin in each
/// zone that this connection has received, or sent and had answered `Ok`, and
/// the zones live on it (F13).
pub struct Subscribes<S> {
    /// Sent and not yet acked: the node acks them in the order sent.
    unacked: VecDeque<(Zone, S)>,
    /// Acked as refused, waiting for the reason (R6).
    refused: VecDeque<(Zone, S)>,
    /// Acked, waiting for the replay to reach each head the ack names (R7).
    catching: Vec<(Zone, Vec<Head>, S)>,
    seen: HashMap<(Zone, String), i64>,
    /// Named by an ack on this connection, and not refused since (F13).
    live: Vec<Zone>,
}

impl<S> Default for Subscribes<S> {
    fn default() -> Self {
        Subscribes {
            unacked: VecDeque::new(),
            refused: VecDeque::new(),
            catching: Vec::new(),
            seen: HashMap::new(),
            live: Vec::new(),
        }
    }
}

impl<S> Subscribes<S> {
    /// A subscribe sent, to be acked after those sent before it.
    pub fn sent(&mut self, zone: Zone, waiter: S) {
        self.unacked.push_back((zone, waiter));
    }

    /// The ack of the oldest subscribe. One naming its zone waits for its
    /// replay, over at once if the connection has seen every head it names.
    /// One naming no zone is a refusal, which waits for its reason (R6). One
    /// naming another zone leaves no subscribe matched to its answer, so every
    /// one waiting is abandoned. Whichever it answers, the zone an ack names
    /// is live: the node has registered the connection to it (F13).
    pub fn acked(&mut self, ack: &Heads) -> Result<Option<Subscribed<S>>, Abandoned<S>> {
        if let Some(named) = ack.streams.first() {
            let (share, glade_id) = (named.share.clone(), named.glade_id.clone());
            let zone = (share, glade_id, named.key.clone());
            if !self.live.contains(&zone) {
                self.live.push(zone);
            }
        }
        let Some((zone, waiter)) = self.unacked.pop_front() else {
            return Ok(None);
        };
        let Some(named) = ack.streams.first() else {
            self.refused.push_back((zone, waiter));
            return Ok(None);
        };
        if named.share != zone.0 || named.glade_id != zone.1 || named.key != zone.2 {
            self.unacked.push_front((zone, waiter));
            return Err(self.abandon());
        }
        self.catching.push((zone.clone(), named.heads.clone(), waiter));
        Ok(self.caught_up(&[zone]).pop())
    }

    /// An `Error` with no `corr` is the reason for the oldest refused subscribe
    /// of its share and stream (R6).
    pub fn reason(&mut self, status: &Error) -> Option<Subscribed<S>> {
        if status.corr.is_some() {
            return None;
        }
        let named = |zone: &Zone| names(status, zone);
        let at = self.refused.iter().position(|(zone, _)| named(zone))?;
        let (zone, waiter) = self.refused.remove(at)?;
        let outcome = SubscribeOutcome::Refused { code: Some(status.code), message: status.message.clone() };
        Some(Subscribed { zone, outcome, waiter })
    }

    /// F13: an `Error` naming no op, and no refused subscribe's reason (R6),
    /// refuses zones after their ack. It names no key, so it refuses each
    /// live zone of its share and stream: each leaves the live zones, and a
    /// subscribe of one still waiting for its replay, which cannot come now,
    /// is refused with the same code and reason.
    pub fn refused_after_ack(&mut self, status: &Error) -> RefusedAfterAck<S> {
        let reason = self.refused.iter().any(|(zone, _)| names(status, zone));
        if status.corr.is_some() || reason {
            let (zones, subscribes) = (Vec::new(), Vec::new());
            return RefusedAfterAck { zones, subscribes };
        }
        let (zones, live): (Vec<_>, Vec<_>) = std::mem::take(&mut self.live)
            .into_iter()
            .partition(|zone| names(status, zone));
        self.live = live;
        let (waiting, catching): (Vec<_>, Vec<_>) = std::mem::take(&mut self.catching)
            .into_iter()
            .partition(|(zone, _, _)| names(status, zone));
        self.catching = catching;
        let outcome = SubscribeOutcome::Refused {
            code: Some(status.code),
            message: status.message.clone(),
        };
        let subscribes = waiting.into_iter().map(|(zone, _, waiter)| Subscribed {
            zone,
            outcome: outcome.clone(),
            waiter,
        });
        let zones = zones.into_iter().map(|(share, glade_id, key)| ZoneRefusal {
            share,
            glade_id,
            key,
            code: status.code,
            message: status.message.clone(),
        });
        let (zones, subscribes) = (zones.collect(), subscribes.collect());
        RefusedAfterAck { zones, subscribes }
    }

    /// Whether the zone is live on this connection: named by an ack, and not
    /// refused since (F13).
    pub fn live(&self, zone: &Zone) -> bool {
        self.live.contains(zone)
    }

    /// Ops this connection received: the subscribes whose replay they complete.
    pub fn received(&mut self, ops: &[Op]) -> Vec<Subscribed<S>> {
        let mut zones: Vec<Zone> = Vec::new();
        for op in ops {
            self.see(op);
            let zone = zone_of(op);
            if !zones.contains(&zone) {
                zones.push(zone);
            }
        }
        self.caught_up(&zones)
    }

    /// An op this connection sent, and its answer. One the node holds counts
    /// toward a replay, since the node does not send it back (R4, R7).
    pub fn answered(&mut self, op: &Op, outcome: &OpOutcome) -> Vec<Subscribed<S>> {
        if *outcome != OpOutcome::Accepted {
            return Vec::new();
        }
        self.see(op);
        self.caught_up(&[zone_of(op)])
    }

    /// A frame the session could not take: the replays of its zones cannot
    /// complete, so their subscribes fail.
    pub fn failed(&mut self, ops: &[Op]) -> Vec<S> {
        let zones: Vec<Zone> = ops.iter().map(zone_of).collect();
        let (failed, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.catching).into_iter().partition(|(zone, _, _)| zones.contains(zone));
        self.catching = waiting;
        failed.into_iter().map(|(_, _, waiter)| waiter).collect()
    }

    /// A frame the client could not take (CD-G4): it may have been any
    /// waiting subscribe's ack, or carried its replay, so every waiting
    /// subscribe is abandoned. The connection goes on, and what it saw stays.
    pub fn not_taken(&mut self) -> Abandoned<S> {
        self.abandon()
    }

    /// The connection ended: every waiting subscribe is abandoned, and what
    /// the connection saw goes with it, its live zones too.
    pub fn ended(&mut self) -> Abandoned<S> {
        self.seen.clear();
        self.live.clear();
        self.abandon()
    }

    fn abandon(&mut self) -> Abandoned<S> {
        let unacked = self.unacked.drain(..).map(|(_, waiter)| waiter);
        let catching = self.catching.drain(..).map(|(_, _, waiter)| waiter);
        let failed = unacked.chain(catching).collect();
        let unknown = || SubscribeOutcome::Refused { code: None, message: "its reason did not come".into() };
        let refused = self.refused.drain(..).map(|(zone, waiter)| Subscribed { zone, outcome: unknown(), waiter }).collect();
        Abandoned { failed, refused }
    }

    fn see(&mut self, op: &Op) {
        let seen = self.seen.entry((zone_of(op), op.origin.clone())).or_insert(op.seq);
        *seen = (*seen).max(op.seq);
    }

    /// The subscribes of `zones` whose replay is in: the connection has seen
    /// an op at or above every head their ack names (R7).
    fn caught_up(&mut self, zones: &[Zone]) -> Vec<Subscribed<S>> {
        let mut done = Vec::new();
        let mut at = 0;
        while at < self.catching.len() {
            let (zone, heads, _) = &self.catching[at];
            let reached = |head: &Head| self.seen.get(&(zone.clone(), head.origin.clone())).is_some_and(|seq| *seq >= head.seq);
            if zones.contains(zone) && heads.iter().all(reached) {
                let (zone, heads, waiter) = self.catching.remove(at);
                done.push(Subscribed { zone, outcome: SubscribeOutcome::Accepted { heads }, waiter });
            } else {
                at += 1;
            }
        }
        done
    }
}

/// W5's backoff: 1 s, doubling, to 30 s.
pub fn resend_delay(attempt: u32) -> Duration {
    Duration::from_secs(2u64.saturating_pow(attempt).min(30))
}

fn unknown<W>(sent: Sent<W>) -> Answered<W> {
    Answered { op: sent.op, outcome: OpOutcome::Unknown, waiter: sent.waiter, newly_unplaced: false }
}

/// Whether an `Error` naming no op names `zone`: by its share and stream,
/// since the wire's `Error` carries no key.
fn names(status: &Error, (share, glade_id, _): &Zone) -> bool {
    status.share.as_ref() == Some(share) && status.glade_id.as_ref() == Some(glade_id)
}

fn in_zone(op: &Op, (share, glade_id, key): &Zone) -> bool {
    op.share == *share && op.glade_id == *glade_id && op.key == *key
}

fn same_chain(a: &Op, b: &Op) -> bool {
    a.share == b.share && a.glade_id == b.glade_id && a.key == b.key && a.origin == b.origin
}

#[cfg(test)]
mod tests {
    use super::*;
    use glade_wire::generated::{Shape, StreamHeads};

    /// A status as the node sends it (R1): the op's share and stream, and its
    /// hash in lower-case hex as `corr`.
    fn status(op: &Op, code: ErrorCode) -> Error {
        let corr = op_hash(op).iter().map(|b| format!("{b:02x}")).collect();
        Error { code, message: format!("{code:?}"), share: Some(op.share.clone()), glade_id: Some(op.glade_id.clone()), corr: Some(corr) }
    }

    /// Three value ops on one chain of `w`'s, sent in order; the waiter of op
    /// `i` is `i`.
    fn three(session: &mut Session, answers: &mut Answers<usize>) -> Vec<Op> {
        let mut ops = Vec::new();
        for i in 0..3u8 {
            let op = session.append("s", "g", Shape::Value, vec![i], vec![]).unwrap();
            assert!(answers.sent(op.clone(), Some(i as usize)).is_none());
            ops.push(op);
        }
        ops
    }

    #[test]
    fn a_status_resolves_its_op() {
        let mut session = Session::new("w");
        let mut answers = Answers::new(WAITING_BOUND);
        let ops = three(&mut session, &mut answers);

        let ok = answers.status(&mut session, &status(&ops[0], ErrorCode::Ok)).expect("the Ok status resolves its op");
        assert_eq!(ok, Answered { op: ops[0].clone(), outcome: OpOutcome::Accepted, waiter: Some(0), newly_unplaced: false });
        // `Retention` is settled, not refused: nothing is dropped.
        let old = answers.status(&mut session, &status(&ops[1], ErrorCode::Retention)).unwrap();
        assert_eq!((old.outcome, old.waiter), (OpOutcome::Retained, Some(1)));
        assert_eq!(session.fold_value("s", "g", &[]), Some(vec![2]), "a settled op drops nothing");
        // An op sent twice takes its two statuses in order.
        assert!(answers.sent(ops[2].clone(), Some(3)).is_none());
        let first = answers.status(&mut session, &status(&ops[2], ErrorCode::Ok)).unwrap();
        let second = answers.status(&mut session, &status(&ops[2], ErrorCode::Ok)).unwrap();
        assert_eq!((first.waiter, second.waiter), (Some(2), Some(3)), "statuses keep the order of the sends");
        assert_eq!(answers.waiting(), 0);
    }

    #[test]
    fn a_refusal_resolves_its_op_as_refused_and_drops_the_tail() {
        let mut session = Session::new("w");
        let mut answers = Answers::new(WAITING_BOUND);
        let ops = three(&mut session, &mut answers);
        answers.status(&mut session, &status(&ops[0], ErrorCode::Ok)).unwrap();

        let refused = answers.status(&mut session, &status(&ops[1], ErrorCode::Equivocation)).unwrap();
        let said = OpOutcome::Refused { code: ErrorCode::Equivocation, message: "Equivocation".into() };
        assert_eq!((refused.outcome, refused.waiter), (said, Some(1)));
        assert_eq!(session.fold_value("s", "g", &[]), Some(vec![0]), "the refused op and the one after it are dropped");
        let stopped = session.append("s", "g", Shape::Value, vec![9], vec![]);
        assert!(stopped.is_err(), "the chain waits for a subscribe: {stopped:?}");

        // A subscribe ack for the zone lets it go on, from the op the node holds.
        session.resumed("s", "g", &[]);
        let next = session.append("s", "g", Shape::Value, vec![9], vec![]).unwrap();
        assert_eq!((next.seq, next.prev), (1, Some(op_hash(&ops[0]).to_vec())));
        // The tail's own refusal is still a refusal, and drops nothing more:
        // the session no longer holds that op.
        let tail = answers.status(&mut session, &status(&ops[2], ErrorCode::Protocol)).unwrap();
        assert!(matches!(tail.outcome, OpOutcome::Refused { code: ErrorCode::Protocol, .. }), "{tail:?}");
        assert_eq!(session.fold_value("s", "g", &[]), Some(vec![9]));
        assert_eq!(session.append("s", "g", Shape::Value, vec![10], vec![]).unwrap().seq, 2);
    }

    #[test]
    fn an_unknown_hash_is_ignored() {
        let mut session = Session::new("w");
        let mut answers = Answers::new(WAITING_BOUND);
        let sent = session.append("s", "g", Shape::Value, b"sent".to_vec(), vec![]).unwrap();
        answers.sent(sent.clone(), Some(0));
        let never = Session::new("other").append("s", "g", Shape::Value, b"never sent".to_vec(), vec![]).unwrap();

        let stray = answers.status(&mut session, &status(&never, ErrorCode::Equivocation));
        assert!(stray.is_none(), "a status for an op never sent resolves nothing: {stray:?}");
        // R6's reason names no op.
        let reason = Error { corr: None, ..status(&never, ErrorCode::UnknownShare) };
        assert!(answers.status(&mut session, &reason).is_none());
        assert_eq!(session.fold_value("s", "g", &[]), Some(b"sent".to_vec()), "nothing was dropped");
        let ok = answers.status(&mut session, &status(&sent, ErrorCode::Ok)).unwrap();
        assert_eq!((ok.outcome, ok.waiter), (OpOutcome::Accepted, Some(0)));
    }

    #[test]
    fn the_connections_end_resolves_every_waiting_op_as_unknown() {
        let mut session = Session::new("w");
        let mut answers = Answers::new(WAITING_BOUND);
        let ops = three(&mut session, &mut answers);
        answers.status(&mut session, &status(&ops[0], ErrorCode::Ok)).unwrap();

        let ended = answers.ended();
        let told: Vec<_> = ended.iter().map(|a| (a.op.seq, a.outcome.clone(), a.waiter)).collect();
        assert_eq!(told, vec![(1, OpOutcome::Unknown, Some(1)), (2, OpOutcome::Unknown, Some(2))], "every waiting op is answered Unknown");
        assert_eq!(answers.waiting(), 0);
        // A status after the end finds nothing waiting, and the session keeps its ops.
        assert!(answers.status(&mut session, &status(&ops[1], ErrorCode::Equivocation)).is_none());
        assert_eq!(session.fold_value("s", "g", &[]), Some(vec![2]));
    }

    #[test]
    fn the_fire_and_forget_table_is_bounded() {
        let mut session = Session::new("w");
        let mut answers = Answers::new(3);
        let ops: Vec<Op> = (0..5u8).map(|i| session.append("s", "g", Shape::Log, vec![i], vec![]).unwrap()).collect();
        assert!(answers.sent(ops[0].clone(), Some(0)).is_none());
        for op in &ops[1..3] {
            assert!(answers.sent(op.clone(), None).is_none());
        }

        // A node from before Phase 2 answers nothing: past the bound the oldest
        // send is let go, and its waiter hears `Unknown`.
        let gone = answers.sent(ops[3].clone(), None);
        assert_eq!(gone, Some(Answered { op: ops[0].clone(), outcome: OpOutcome::Unknown, waiter: Some(0), newly_unplaced: false }));
        answers.sent(ops[4].clone(), None);
        assert_eq!(answers.waiting(), 3, "the table keeps no more than its bound");
        assert!(answers.status(&mut session, &status(&ops[1], ErrorCode::Ok)).is_none(), "a let-go op is not answered");
        assert!(answers.status(&mut session, &status(&ops[4], ErrorCode::Ok)).is_some());
    }

    // ---- W5: an op answered `UnknownShare` is not placed (X3.3a) ----------

    /// The zone the ops of `three` are in.
    fn zone() -> Zone {
        ("s".into(), "g".into(), vec![])
    }

    #[test]
    fn unplaced_is_not_refused() {
        let mut session = Session::new("w");
        let mut answers = Answers::new(WAITING_BOUND);
        let ops = three(&mut session, &mut answers);

        let unplaced = answers.status(&mut session, &status(&ops[0], ErrorCode::UnknownShare)).unwrap();
        let said = OpOutcome::NotPlaced { message: "UnknownShare".into() };
        assert_eq!((unplaced.outcome, unplaced.waiter, unplaced.newly_unplaced), (said, Some(0), true), "UnknownShare is not placed, not refused");
        answers.status(&mut session, &status(&ops[1], ErrorCode::UnknownShare)).unwrap();
        // The session keeps the ops and their chain, and `append` goes on.
        assert_eq!(session.fold_value("s", "g", &[]), Some(vec![2]));
        assert_eq!(session.append("s", "g", Shape::Value, vec![3], vec![]).unwrap().seq, 3);
        // Both go again, in their chain's order, after a subscribe of the
        // zone and on the zone's timer.
        assert_eq!(answers.unplaced_in(&zone()), ops[..2].to_vec());
        let timer = answers.start_resending(&zone()).expect("the zone's resend timer starts");
        assert_eq!(answers.start_resending(&zone()), None, "one timer a zone");
        assert_eq!(answers.resend_tick(&zone(), timer), Some(ops[..2].to_vec()));
        // Not placed again, an op is not told twice.
        answers.sent(ops[0].clone(), None);
        let again = answers.status(&mut session, &status(&ops[0], ErrorCode::UnknownShare)).unwrap();
        assert!(!again.newly_unplaced, "told once");
        // The connection's end stops the timer and keeps the ops.
        answers.ended();
        assert_eq!(answers.resend_tick(&zone(), timer), None, "no timer runs past the connection's end");
        assert_eq!(answers.unplaced_in(&zone()), ops[..2].to_vec());
    }

    #[test]
    fn a_repeats_ok_settles_an_unplaced_op() {
        let mut session = Session::new("w");
        let mut answers = Answers::new(WAITING_BOUND);
        let ops = three(&mut session, &mut answers);
        answers.status(&mut session, &status(&ops[0], ErrorCode::UnknownShare)).unwrap();
        let timer = answers.start_resending(&zone()).unwrap();

        // Sent again, it waits for its status, so it is not sent a third time.
        for op in answers.unplaced_in(&zone()) {
            answers.sent(op, None);
        }
        assert_eq!(answers.resend_tick(&zone(), timer), Some(vec![]), "an op sent again and waiting is not sent again");
        let settled = answers.status(&mut session, &status(&ops[0], ErrorCode::Ok)).unwrap();
        assert_eq!((settled.outcome, settled.waiter), (OpOutcome::Accepted, None));
        assert!(answers.unplaced_in(&zone()).is_empty(), "the repeat's Ok settles it");
        assert_eq!(answers.resend_tick(&zone(), timer), None, "with every op placed, the zone's timer stops");
        assert_eq!(answers.start_resending(&zone()), None);
    }

    #[test]
    fn a_later_refusal_still_drops_the_tail() {
        let mut session = Session::new("w");
        let mut answers = Answers::new(WAITING_BOUND);
        let ops = three(&mut session, &mut answers);
        answers.status(&mut session, &status(&ops[0], ErrorCode::Ok)).unwrap();
        answers.status(&mut session, &status(&ops[1], ErrorCode::UnknownShare)).unwrap();
        answers.status(&mut session, &status(&ops[2], ErrorCode::UnknownShare)).unwrap();

        // Sent again, op 1 is refused: another writer took its seq meanwhile.
        for op in answers.unplaced_in(&zone()) {
            answers.sent(op, None);
        }
        let refused = answers.status(&mut session, &status(&ops[1], ErrorCode::Equivocation)).unwrap();
        assert!(matches!(refused.outcome, OpOutcome::Refused { code: ErrorCode::Equivocation, .. }), "{refused:?}");
        assert_eq!(session.fold_value("s", "g", &[]), Some(vec![0]), "the refused op and its tail are dropped");
        assert!(session.append("s", "g", Shape::Value, vec![9], vec![]).is_err(), "the chain waits for a subscribe");
        answers.ended();
        assert!(answers.unplaced_in(&zone()).is_empty(), "a dropped op is never sent again");
    }

    // ---- F6: never past an unplaced op of the same chain ------------------

    #[test]
    fn a_gap_past_an_unplaced_op_is_not_placed() {
        let mut session = Session::new("w");
        let mut answers = Answers::new(WAITING_BOUND);
        let ops = three(&mut session, &mut answers);
        answers
            .status(&mut session, &status(&ops[0], ErrorCode::UnknownShare))
            .unwrap();

        // Ops 1 and 2 went out before the client knew, and met the gap op 0
        // left. By their place, they are not placed either.
        let gap = answers
            .status(&mut session, &status(&ops[1], ErrorCode::Protocol))
            .unwrap();
        let said = OpOutcome::NotPlaced {
            message: "Protocol".into(),
        };
        let got = (gap.outcome, gap.waiter, gap.newly_unplaced);
        assert_eq!(
            got,
            (said, Some(1), true),
            "a gap past an unplaced op is no refusal"
        );
        answers
            .status(&mut session, &status(&ops[2], ErrorCode::Protocol))
            .unwrap();
        // Another chain's `Protocol` in the zone is still a refusal.
        let theirs = Session::new("v")
            .append("s", "g", Shape::Value, vec![9], vec![])
            .unwrap();
        answers.sent(theirs.clone(), None);
        let other = answers
            .status(&mut session, &status(&theirs, ErrorCode::Protocol))
            .unwrap();
        assert!(
            matches!(other.outcome, OpOutcome::Refused { .. }),
            "{other:?}"
        );
        // Answer 4 does not apply: the session keeps the chain, and all three
        // go again, in order.
        assert_eq!(session.fold_value("s", "g", &[]), Some(vec![2]));
        assert_eq!(answers.unplaced_in(&zone()), ops);

        // Sent again, op 0 is placed. Op 1's `Protocol` then follows no op not
        // placed: a real conflict, which answer 4 drops with its tail.
        for op in answers.unplaced_in(&zone()) {
            answers.sent(op, None);
        }
        answers
            .status(&mut session, &status(&ops[0], ErrorCode::Ok))
            .unwrap();
        let refused = answers
            .status(&mut session, &status(&ops[1], ErrorCode::Protocol))
            .unwrap();
        let protocol = matches!(
            refused.outcome,
            OpOutcome::Refused {
                code: ErrorCode::Protocol,
                ..
            }
        );
        assert!(protocol, "{refused:?}");
        assert_eq!(
            session.fold_value("s", "g", &[]),
            Some(vec![0]),
            "the refused op and its tail are dropped"
        );
        answers.ended();
        assert!(
            answers.unplaced_in(&zone()).is_empty(),
            "a dropped op is never sent again"
        );
    }

    #[test]
    fn no_op_goes_past_an_unplaced_op_of_its_chain() {
        let mut session = Session::new("w");
        let mut answers = Answers::new(WAITING_BOUND);
        let append = |session: &mut Session, glade_id: &str, i: u8| {
            session
                .append("s", glade_id, Shape::Value, vec![i], vec![])
                .unwrap()
        };
        let first = append(&mut session, "g", 0);
        assert_eq!(
            answers.to_send(vec![first.clone()], vec![Some(0)]).0,
            vec![first.clone()]
        );
        answers
            .status(&mut session, &status(&first, ErrorCode::UnknownShare))
            .unwrap();

        // While op 0 is not placed, a later op of its chain is held, not sent.
        // Its waiter hears it is not placed, and it is told once.
        let later = append(&mut session, "g", 1);
        let (now, told) = answers.to_send(vec![later.clone()], vec![Some(1)]);
        assert!(
            now.is_empty(),
            "no op goes past an unplaced op of its chain: {now:?}"
        );
        let held = OpOutcome::NotPlaced {
            message: "not sent: seq 0 of its chain is not placed".into(),
        };
        let told: Vec<_> = told
            .into_iter()
            .map(|a| (a.op, a.outcome, a.waiter, a.newly_unplaced))
            .collect();
        assert_eq!(told, vec![(later.clone(), held, Some(1), true)]);
        // Another chain goes at once: another zone, or another origin here.
        let elsewhere = append(&mut session, "h", 2);
        let theirs = append(&mut Session::new("v"), "g", 3);
        let (now, _) = answers.to_send(vec![elsewhere.clone(), theirs.clone()], vec![None, None]);
        assert_eq!(now, vec![elsewhere, theirs]);

        // Sent again, op 0 goes first, and the held op after it.
        let again = answers.unplaced_in(&zone());
        assert_eq!(again, vec![first.clone(), later.clone()]);
        assert_eq!(
            answers.to_send(again, vec![None, None]).0,
            vec![first.clone(), later]
        );
        // With op 0 sent again, sending has resumed: the next op goes at once.
        let next = append(&mut session, "g", 4);
        assert_eq!(
            answers.to_send(vec![next.clone()], vec![None]).0,
            vec![next]
        );
        // Not placed once more, op 0 holds its chain back again.
        answers
            .status(&mut session, &status(&first, ErrorCode::UnknownShare))
            .unwrap();
        let last = append(&mut session, "g", 5);
        assert!(
            answers.to_send(vec![last], vec![None]).0.is_empty(),
            "held again"
        );
    }

    #[test]
    fn the_resend_backoff_doubles_from_one_second_to_thirty_in_each_zone() {
        let delays: Vec<u64> = (0..8).map(|attempt| resend_delay(attempt).as_secs()).collect();
        assert_eq!(delays, vec![1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(resend_delay(u32::MAX), Duration::from_secs(30));

        // Each zone counts its own resends, from when it last had every op placed.
        let mut session = Session::new("w");
        let mut answers = Answers::<usize>::new(WAITING_BOUND);
        let a = session.append("s", "a", Shape::Value, vec![0], vec![]).unwrap();
        let b = session.append("s", "b", Shape::Value, vec![0], vec![]).unwrap();
        for op in [&a, &b] {
            answers.sent(op.clone(), None);
            answers.status(&mut session, &status(op, ErrorCode::UnknownShare)).unwrap();
        }
        assert!(answers.start_resending(&zone_of(&a)).is_some());
        assert!(answers.start_resending(&zone_of(&b)).is_some(), "each zone runs its own timer");
        let waits: Vec<u64> = (0..3).map(|_| answers.next_resend(&zone_of(&a)).as_secs()).collect();
        assert_eq!(waits, vec![1, 2, 4]);
        assert_eq!(answers.next_resend(&zone_of(&b)).as_secs(), 1, "another zone keeps its own count");
        answers.sent(a.clone(), None);
        answers.status(&mut session, &status(&a, ErrorCode::Ok)).unwrap();
        assert_eq!(answers.next_resend(&zone_of(&a)).as_secs(), 1, "a zone with every op placed starts again");
    }

    // ---- Catching up: the subscribe outcome (Step 3.2) --------------------

    /// An op of `origin` at `seq` in `zone`.
    fn op_in(zone: &Zone, origin: &str, seq: i64) -> Op {
        Op { share: zone.0.clone(), glade_id: zone.1.clone(), key: zone.2.clone(), origin: origin.into(), seq, prev: None, lamport: 0, refs: vec![], shape: Shape::Value, payload: vec![] }
    }

    /// The node's ack of a subscribe to `zone`, naming each origin's head (R5).
    fn ack(zone: &Zone, heads: &[(&str, i64)]) -> Heads {
        let heads = heads.iter().map(|(origin, seq)| Head { origin: (*origin).into(), seq: *seq, hash: None }).collect();
        Heads { streams: vec![StreamHeads { share: zone.0.clone(), glade_id: zone.1.clone(), key: zone.2.clone(), heads }] }
    }

    fn other() -> Zone {
        ("s".into(), "other".into(), vec![])
    }

    #[test]
    fn an_ack_with_no_origins_completes_at_once() {
        let mut subs = Subscribes::default();
        subs.sent(zone(), 0);
        let done = subs.acked(&ack(&zone(), &[]));
        let empty = SubscribeOutcome::Accepted { heads: vec![] };
        assert_eq!(done, Ok(Some(Subscribed { zone: zone(), outcome: empty, waiter: 0 })), "an ack with no origins completes at once");
    }

    #[test]
    fn ops_below_at_and_above_the_acked_seq() {
        let mut subs = Subscribes::default();
        subs.sent(zone(), 0);
        let named = ack(&zone(), &[("a", 5), ("b", 2)]);
        assert_eq!(subs.acked(&named), Ok(None), "nothing seen yet");
        assert!(subs.received(&[op_in(&zone(), "a", 3)]).is_empty(), "below a's head");
        assert!(subs.received(&[op_in(&other(), "a", 9)]).is_empty(), "another zone's op");
        assert!(subs.received(&[op_in(&zone(), "b", 4)]).is_empty(), "above b's head, with a's not reached");
        let done = subs.received(&[op_in(&zone(), "a", 5)]);
        assert_eq!(done.len(), 1, "at a's head, every head is reached");
        assert_eq!(done[0].outcome, SubscribeOutcome::Accepted { heads: named.streams[0].heads.clone() });
    }

    #[test]
    fn the_sessions_own_accepted_ops_count() {
        let mut subs = Subscribes::default();
        subs.sent(zone(), 0);
        subs.acked(&ack(&zone(), &[("w", 1)])).unwrap();
        let own = op_in(&zone(), "w", 1);
        let refused = OpOutcome::Refused { code: ErrorCode::Equivocation, message: String::new() };
        for unheld in [refused, OpOutcome::Retained, OpOutcome::NotPlaced { message: String::new() }, OpOutcome::Unknown] {
            assert!(subs.answered(&own, &unheld).is_empty(), "an op the node does not hold does not count: {unheld:?}");
        }
        let done = subs.answered(&own, &OpOutcome::Accepted);
        assert_eq!(done.len(), 1, "an own op the node holds counts, since the node does not send it back");
    }

    #[test]
    fn an_ack_that_names_no_zone_is_a_refusal() {
        let attic: Zone = ("ws-attic".into(), "g".into(), vec![]);
        let reason = |zone: &Zone, corr: Option<String>| Error { code: ErrorCode::UnknownShare, message: "no live claim".into(), share: Some(zone.0.clone()), glade_id: Some(zone.1.clone()), corr };
        let mut subs = Subscribes::default();
        subs.sent(attic.clone(), 0);
        subs.sent(zone(), 1);
        assert_eq!(subs.acked(&Heads { streams: vec![] }), Ok(None), "a refusal waits for its reason");
        assert!(subs.reason(&reason(&attic, Some("ab".into()))).is_none(), "an op's status is no reason");
        assert!(subs.reason(&reason(&zone(), None)).is_none(), "nor is an Error for a stream no refusal waits on");
        let refused = subs.reason(&reason(&attic, None)).expect("the next Error for the zone with no corr is the reason");
        let said = SubscribeOutcome::Refused { code: Some(ErrorCode::UnknownShare), message: "no live claim".into() };
        assert_eq!(refused, Subscribed { zone: attic, outcome: said, waiter: 0 });
        let next = subs.acked(&ack(&zone(), &[])).unwrap().map(|done| done.waiter);
        assert_eq!(next, Some(1), "the next ack is the next subscribe's");
    }

    #[test]
    fn an_ack_for_another_zone_abandons_every_waiting_subscribe() {
        let attic: Zone = ("ws-attic".into(), "g".into(), vec![]);
        let mut subs = Subscribes::default();
        subs.sent(attic.clone(), 0);
        subs.acked(&Heads { streams: vec![] }).unwrap();
        subs.sent(zone(), 1);
        subs.sent(other(), 2);
        let unknown = SubscribeOutcome::Refused { code: None, message: "its reason did not come".into() };
        let abandoned = Abandoned { failed: vec![1, 2], refused: vec![Subscribed { zone: attic, outcome: unknown, waiter: 0 }] };
        assert_eq!(subs.acked(&ack(&other(), &[])), Err(abandoned), "no subscribe is matched to its answer any more");
    }

    #[test]
    fn a_frame_the_session_cannot_take_fails_its_zones_subscribes() {
        let mut subs = Subscribes::default();
        subs.sent(zone(), 0);
        subs.sent(other(), 1);
        subs.acked(&ack(&zone(), &[("a", 0)])).unwrap();
        subs.acked(&ack(&other(), &[("a", 0)])).unwrap();
        assert_eq!(subs.failed(&[op_in(&zone(), "a", 0)]), vec![0], "the subscribe waiting on the frame's zone fails");
        assert_eq!(subs.received(&[op_in(&other(), "a", 0)]).len(), 1, "another zone's still completes");
    }

    /// CD-G4: a frame the client could not take may have been any waiting
    /// subscribe's ack, or carried its replay, so each is abandoned, as at
    /// the connection's end: those not yet acked and those catching up fail,
    /// and one refused and waiting for its reason stays a refusal, the reason
    /// unknown. The connection goes on, so what it saw stays: its live zones,
    /// and the ops it has seen count toward a later subscribe's replay.
    #[test]
    fn a_frame_not_taken_abandons_every_waiting_subscribe_and_keeps_what_was_seen() {
        let attic: Zone = ("ws-attic".into(), "g".into(), vec![]);
        let mut subs = Subscribes::default();
        subs.received(&[op_in(&zone(), "a", 5)]);
        subs.sent(attic.clone(), 0);
        subs.acked(&Heads { streams: vec![] }).unwrap();
        subs.sent(zone(), 1);
        subs.acked(&ack(&zone(), &[("b", 1)])).unwrap();
        subs.sent(other(), 2);
        let unknown = SubscribeOutcome::Refused {
            code: None,
            message: "its reason did not come".into(),
        };
        let refused = vec![Subscribed {
            zone: attic,
            outcome: unknown,
            waiter: 0,
        }];
        assert_eq!(
            subs.not_taken(),
            Abandoned {
                failed: vec![2, 1],
                refused
            }
        );
        assert!(subs.live(&zone()), "a zone its ack named stays live");
        subs.sent(zone(), 3);
        let done = subs
            .acked(&ack(&zone(), &[("a", 5)]))
            .unwrap()
            .map(|done| done.waiter);
        assert_eq!(done, Some(3), "the ops seen before still count");
    }

    #[test]
    fn the_connections_end_fails_every_waiting_subscribe() {
        let mut subs = Subscribes::default();
        subs.received(&[op_in(&zone(), "a", 5)]);
        for waiter in 0..3 {
            subs.sent(zone(), waiter);
        }
        subs.acked(&Heads { streams: vec![] }).unwrap();
        subs.acked(&ack(&zone(), &[("b", 1)])).unwrap();
        let ended = subs.ended();
        assert_eq!(ended.failed, vec![2, 1], "every waiting subscribe fails");
        let refusals: Vec<_> = ended.refused.iter().map(|refused| (refused.waiter, refused.outcome.clone())).collect();
        let unknown = SubscribeOutcome::Refused { code: None, message: "its reason did not come".into() };
        assert_eq!(refusals, vec![(0, unknown)], "but a refusal stays one, its reason unknown");
        // What the connection saw goes with it.
        subs.sent(zone(), 3);
        assert_eq!(subs.acked(&ack(&zone(), &[("a", 5)])), Ok(None), "a new connection has seen nothing");
    }

    // ---- F13: a zone refused after its ack ---------------------------------

    /// A refusal after the ack, as the node sends it (F5's forward, the grant
    /// re-check pass): an `Error` naming the zone's share and stream, not its
    /// key, and no op.
    fn lone(zone: &Zone) -> Error {
        Error {
            code: ErrorCode::Unauthorized,
            message: "refused by node b, which serves s: unauthorized".into(),
            share: Some(zone.0.clone()),
            glade_id: Some(zone.1.clone()),
            corr: None,
        }
    }

    /// `lone`'s refusal of `zone`, as `on_zone_refused` reports it.
    fn refusal(zone: &Zone) -> ZoneRefusal {
        let (share, glade_id, key) = zone.clone();
        let code = ErrorCode::Unauthorized;
        let message = "refused by node b, which serves s: unauthorized".into();
        ZoneRefusal {
            share,
            glade_id,
            key,
            code,
            message,
        }
    }

    /// A subscribe to `zone`, acked with an empty replay, so it returns at once.
    fn subscribed(subs: &mut Subscribes<usize>, zone: &Zone, waiter: usize) {
        subs.sent(zone.clone(), waiter);
        let done = subs.acked(&ack(zone, &[])).unwrap();
        assert_eq!(done.map(|done| done.waiter), Some(waiter));
    }

    #[test]
    fn a_lone_error_after_an_ack_refuses_its_zone() {
        let mut subs = Subscribes::default();
        subscribed(&mut subs, &zone(), 0);
        assert!(subs.live(&zone()), "a zone is live from its ack");

        let refused = subs.refused_after_ack(&lone(&zone()));
        assert_eq!(refused.zones, vec![refusal(&zone())]);
        let waiting = refused.subscribes;
        assert!(waiting.is_empty(), "its subscribe returned at the ack");
        assert!(!subs.live(&zone()), "a refused zone is no longer live");
        let again = subs.refused_after_ack(&lone(&zone()));
        assert!(again.zones.is_empty(), "told once: {again:?}");
        // An op's status names an op, not a zone.
        subscribed(&mut subs, &other(), 1);
        let status = Error {
            corr: Some("ab".into()),
            ..lone(&other())
        };
        assert!(subs.refused_after_ack(&status).zones.is_empty());
        assert!(subs.live(&other()));
    }

    #[test]
    fn a_refusal_after_the_ack_refuses_a_subscribe_still_waiting_for_its_replay() {
        let mut subs = Subscribes::default();
        subs.sent(zone(), 0);
        let acked = subs.acked(&ack(&zone(), &[("a", 5)]));
        assert_eq!(acked, Ok(None), "its replay is not in");

        let refused = subs.refused_after_ack(&lone(&zone()));
        let (zones, subscribes) = (refused.zones, refused.subscribes);
        let code = Some(ErrorCode::Unauthorized);
        let message = refusal(&zone()).message;
        let outcome = SubscribeOutcome::Refused { code, message };
        let waiting = Subscribed {
            zone: zone(),
            outcome,
            waiter: 0,
        };
        assert_eq!(subscribes, vec![waiting], "its replay cannot come now");
        assert_eq!(zones, vec![refusal(&zone())]);
        let late = subs.received(&[op_in(&zone(), "a", 5)]);
        assert!(late.is_empty(), "none is left to complete: {late:?}");
    }

    #[test]
    fn a_lone_error_refuses_each_live_zone_of_its_share_and_stream() {
        let mut subs = Subscribes::default();
        let keyed: Zone = ("s".into(), "g".into(), b"k".to_vec());
        for (waiter, zone) in [zone(), keyed.clone(), other()].iter().enumerate() {
            subscribed(&mut subs, zone, waiter);
        }

        // The wire names no key, so each key's zone of the stream is refused.
        let refused = subs.refused_after_ack(&lone(&zone()));
        assert_eq!(refused.zones, vec![refusal(&zone()), refusal(&keyed)]);
        assert!(!subs.live(&keyed));
        assert!(subs.live(&other()), "another stream's zone stays live");
    }

    #[test]
    fn a_refused_subscribes_reason_comes_before_a_refusal_after_the_ack() {
        let mut subs = Subscribes::default();
        subscribed(&mut subs, &zone(), 0);
        subs.sent(zone(), 1);
        assert_eq!(subs.acked(&Heads { streams: vec![] }), Ok(None));

        // The next Error with no corr for the stream is that refusal's reason
        // (R6), and the live zone stays live.
        let reason = lone(&zone());
        assert!(subs.refused_after_ack(&reason).zones.is_empty());
        assert!(subs.live(&zone()));
        assert_eq!(subs.reason(&reason).map(|refused| refused.waiter), Some(1));
        // With no refusal waiting for its reason, the next one refuses the zone.
        let refused = subs.refused_after_ack(&reason);
        assert_eq!(refused.zones, vec![refusal(&zone())]);
    }

    #[test]
    fn a_zone_is_live_from_any_ack_naming_it_until_refused_or_the_connection_ends() {
        let mut subs = Subscribes::default();
        subscribed(&mut subs, &zone(), 0);
        subs.refused_after_ack(&lone(&zone()));
        // The client keeps no refusal: a later subscribe's ack makes it live again.
        subscribed(&mut subs, &zone(), 1);
        assert!(subs.live(&zone()), "live again");
        // An ack for another zone in a subscribe's turn still names a zone the
        // node registered.
        subs.sent(("s".into(), "g3".into(), vec![]), 2);
        assert!(subs.acked(&ack(&other(), &[])).is_err());
        assert!(subs.live(&other()), "the zone an ack names is live");

        subs.ended();
        let live = [zone(), other()].iter().any(|zone| subs.live(zone));
        assert!(!live, "no subscription outlives its connection");
        assert!(subs.refused_after_ack(&lone(&zone())).zones.is_empty());
    }
}
