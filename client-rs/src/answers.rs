//! The node's answers to this client's ops (GladeSubstrateV1 §6, R1 and R7).
//! Each op the client sends is kept by its hash until a status names it by
//! `corr`, never by its place among other frames. `Ok` and `Retention` settle
//! it; `UnknownShare` leaves it not placed, kept to be sent again, zone by zone
//! (W5); any other code refuses it, and the session drops it with the rest of
//! its chain (answer 4). Pure: no socket and no clock (LBT-008). `client.rs`
//! feeds it the statuses and tells the waiters it hands back.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use glade_wire::generated::{Error, ErrorCode, Op};

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
    /// and sends them again.
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
/// the order their statuses came, with each zone's resend count and timer.
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
            .filter(|op| in_zone(op, zone) && !self.waiting.iter().any(|sent| sent.op == **op))
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
}

/// W5's backoff: 1 s, doubling, to 30 s.
pub fn resend_delay(attempt: u32) -> Duration {
    Duration::from_secs(2u64.saturating_pow(attempt).min(30))
}

fn unknown<W>(sent: Sent<W>) -> Answered<W> {
    Answered { op: sent.op, outcome: OpOutcome::Unknown, waiter: sent.waiter, newly_unplaced: false }
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
    use glade_wire::generated::Shape;

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
}
