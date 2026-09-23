use glade_grant_api::conformance::{self, Record};
use glade_grant_api::{Denial, GrantPort, Holder};

/// Deliberately wrong behaviours, each caught by one probe.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wrong {
    AnyVerb,
    LastWriterWins,
    UnavailableAsNoGrant,
}

/// A volatile in-memory fold over the fixture records. Not the node's registry:
/// it has no chain, origin or persistence.
struct Fold {
    records: Vec<Record>,
    readable: bool,
    wrong: Option<Wrong>,
}

fn fold(readable: bool, wrong: Option<Wrong>) -> Fold {
    Fold {
        records: conformance::fold(),
        readable,
        wrong,
    }
}

impl GrantPort for Fold {
    fn check(&self, holder: &Holder, verb: &str, share: &str) -> Result<(), Denial> {
        if !self.readable {
            if self.wrong == Some(Wrong::UnavailableAsNoGrant) {
                return Err(Denial::NoGrant);
            }
            return Err(Denial::Unavailable);
        }
        let (mut granted, mut revoked) = (false, false);
        for record in &self.records {
            match record {
                Record::Grant {
                    holder: h,
                    share: s,
                    verbs,
                } if h == holder && *s == share => {
                    granted |= verbs.contains(&verb) || self.wrong == Some(Wrong::AnyVerb);
                    revoked &= self.wrong != Some(Wrong::LastWriterWins);
                }
                Record::Revoke {
                    holder: h,
                    share: s,
                } if h == holder && *s == share => {
                    revoked = true;
                }
                _ => {}
            }
        }
        if revoked {
            Err(Denial::Revoked)
        } else if granted {
            Ok(())
        } else {
            Err(Denial::NoGrant)
        }
    }
}

#[test]
fn gr_001_exact_match_implies_nothing() {
    conformance::exact(&fold(true, None));
}

#[test]
fn gr_002_revocation_wins_in_either_order() {
    conformance::revocation_wins(&fold(true, None));
}

#[test]
fn gr_003_an_unreadable_fold_is_unavailable() {
    conformance::unavailable(&fold(false, None));
}

#[test]
#[should_panic(expected = "GR-001 nothing is implied")]
fn rejects_an_implied_verb() {
    conformance::exact(&fold(true, Some(Wrong::AnyVerb)));
}

#[test]
#[should_panic(expected = "GR-002 a revocation before the grant")]
fn rejects_a_grant_that_outlives_its_revocation() {
    conformance::revocation_wins(&fold(true, Some(Wrong::LastWriterWins)));
}

#[test]
#[should_panic(expected = "GR-003")]
fn rejects_unavailable_reported_as_no_grant() {
    conformance::unavailable(&fold(false, Some(Wrong::UnavailableAsNoGrant)));
}
