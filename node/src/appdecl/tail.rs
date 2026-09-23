//! The binding line's keyword tail (R11(a); the reconciliation document's
//! §4.4 bullet 4): `key=value` entries after the five positional tokens, in
//! any order, each key at most once. Two keys exist:
//!
//! - `ttl=<duration>`: a whole number above zero and exactly one unit of `ms`,
//!   `s`, `m`, `h` or `d` (`ttl=10m`), held in milliseconds as the contract's
//!   `Retention.ttl_ms`. Legal only on a line whose retention is `ttl`; a bare
//!   `ttl` still parses and names no duration.
//! - `shape-profile=<profile>`: exactly one of GDL-041's profiles, over its
//!   core engine: `snapshot_delta` (over `swmr`) or `text_crdt` (over `crdt`).
//!   `value` and `log` take none; `swmr` may omit it (the bare engine); `crdt`
//!   must carry it. The key is not `profile`, which already names the node's
//!   boot profile (R11's naming note).
//!
//! The tail starts after the fifth token, so a `key=value` entry standing
//! where the zone or the retention goes means a token is missing, and it is
//! refused rather than stored as that token. Each rule is enforced at parse
//! with the line number, as every binding check is. This is the tail's own
//! grammar, not the zone and retention validation plan Step 2.6 switches on.
//! What the tail yields is parse data ([`BindingTail`]); no record carries it.

use super::BindingTail;

/// The keys a tail may carry.
const KEYS: [&str; 2] = ["ttl", "shape-profile"];

/// GDL-041's profiles, each with the core engine it runs over (the contract
/// IR's `shapes` block, `class: profile`): the only `shape-profile=` values.
const PROFILES: [(&str, &str); 2] = [("snapshot_delta", "swmr"), ("text_crdt", "crdt")];

/// Shapes whose binding must name a profile: glial throws at mount without
/// one, and a file that loads into an unmountable surface is the defect.
const PROFILE_REQUIRED: [&str; 1] = ["crdt"];

/// The units `ttl=` takes, each in milliseconds.
const UNITS: [(&str, i64); 5] =
    [("ms", 1), ("s", 1_000), ("m", 60_000), ("h", 3_600_000), ("d", 86_400_000)];

/// Parse binding line `n`'s tail — `toks`, the tokens after the five
/// positional ones — against the line's shape, zone and retention. `Ok(None)`
/// for a line with no tail that needs none.
pub(super) fn parse(
    n: usize,
    glade_id: &str,
    shape: &str,
    zone: &str,
    retention: &str,
    toks: &[&str],
) -> Result<Option<BindingTail>, String> {
    // Shape and authority are checked against their vocabularies already; the
    // zone and the retention are not (Step 2.6), so a tail entry that slid
    // into either slot would otherwise pass as that token.
    for (slot, tok) in [("zone", zone), ("retention", retention)] {
        if tok.contains('=') {
            return Err(format!(
                "line {n}: `{tok}` is a key=value entry where <{slot}> goes (the tail follows all five tokens)"
            ));
        }
    }
    let mut tail = BindingTail { glade_id: glade_id.into(), ..BindingTail::default() };
    let mut seen: Vec<&str> = Vec::new();
    for tok in toks {
        let Some((key, value)) = tok.split_once('=').filter(|(key, _)| !key.is_empty()) else {
            return Err(format!("line {n}: `{tok}` is not a key=value entry ({})", keys()));
        };
        if !KEYS.contains(&key) {
            return Err(format!("line {n}: unknown binding key `{key}` ({})", keys()));
        }
        if seen.contains(&key) {
            return Err(format!("line {n}: repeated binding key `{key}`"));
        }
        seen.push(key);
        if key == "ttl" {
            tail.ttl_ms = Some(duration_ms(n, value)?);
        } else {
            tail.shape_profile = Some(profile(n, value)?.to_string());
        }
    }
    if tail.ttl_ms.is_some() && retention != "ttl" {
        return Err(format!("line {n}: `ttl=` needs the retention `ttl` (this line's is `{retention}`)"));
    }
    fits_shape(n, shape, tail.shape_profile.as_deref())?;
    if seen.is_empty() {
        Ok(None)
    } else {
        Ok(Some(tail))
    }
}

/// The tail's keys, as a diagnostic lists them.
fn keys() -> String {
    let quoted: Vec<String> = KEYS.iter().map(|k| format!("`{k}`")).collect();
    format!("binding keys: {}", quoted.join(", "))
}

/// GDL-041's profiles, as a diagnostic lists them.
fn profiles() -> String {
    let each: Vec<String> = PROFILES.iter().map(|(p, core)| format!("`{p}` over `{core}`")).collect();
    format!("profiles: {}", each.join(", "))
}

/// `ttl=`'s value in milliseconds: digits, then exactly one unit.
fn duration_ms(n: usize, value: &str) -> Result<i64, String> {
    let bad = || {
        format!(
            "line {n}: bad duration `{value}` (`ttl=` takes a whole number above zero and one unit of ms, s, m, h or d, such as `ttl=10m`)"
        )
    };
    let out_of_range = || format!("line {n}: duration `{value}` is out of range");
    let digits = value.len() - value.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let (number, unit) = value.split_at(digits);
    let Some(&(_, scale)) = UNITS.iter().find(|(u, _)| *u == unit) else {
        return Err(bad());
    };
    if number.is_empty() {
        return Err(bad());
    }
    // All ASCII digits, so the only parse failure is overflow.
    let count: i64 = number.parse().map_err(|_| out_of_range())?;
    if count == 0 {
        return Err(bad());
    }
    count.checked_mul(scale).ok_or_else(out_of_range)
}

/// `shape-profile=`'s value: one of GDL-041's profiles, by exact name.
fn profile(n: usize, value: &str) -> Result<&'static str, String> {
    match PROFILES.iter().find(|(p, _)| *p == value) {
        Some((p, _)) => Ok(p),
        None => Err(format!("line {n}: unknown profile `{value}` ({})", profiles())),
    }
}

/// Does the line's profile (or its absence) fit its shape? A profile runs
/// over one core engine; a shape no profile runs over takes none; a shape in
/// [`PROFILE_REQUIRED`] must name one.
fn fits_shape(n: usize, shape: &str, profile: Option<&str>) -> Result<(), String> {
    let over_shape: Vec<&str> = PROFILES.iter().filter(|(_, core)| *core == shape).map(|(p, _)| *p).collect();
    match profile {
        Some(p) => {
            if over_shape.is_empty() {
                return Err(format!("line {n}: shape `{shape}` takes no `shape-profile` ({})", profiles()));
            }
            if !over_shape.contains(&p) {
                let core = PROFILES.iter().find(|(name, _)| *name == p).map(|(_, core)| *core).unwrap_or("");
                return Err(format!("line {n}: profile `{p}` is over `{core}`, not `{shape}`"));
            }
            Ok(())
        }
        None => {
            if PROFILE_REQUIRED.contains(&shape) {
                let needs: Vec<String> = over_shape.iter().map(|p| format!("`shape-profile={p}`")).collect();
                return Err(format!(
                    "line {n}: shape `{shape}` needs {} (glial cannot mount a {shape} surface without its profile)",
                    needs.join(" or ")
                ));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::appdecl::{parse, AppDecl, BindingTail};

    /// One binding line in a minimal file, where it is line 3.
    fn line(binding: &str) -> Result<AppDecl, String> {
        parse(&format!("glade-app v0\napp x\n{binding}\n"))
    }

    /// The tail is optional and comes after the five positional tokens, its
    /// entries in any order.
    #[test]
    fn a_tail_follows_the_five_tokens_in_any_order() {
        line("binding g value share commons latest").unwrap();
        line("binding g value share commons ttl ttl=10m").unwrap();
        line("binding g swmr share commons from-cursor shape-profile=snapshot_delta").unwrap();
        line("binding g crdt share commons ttl ttl=1h shape-profile=text_crdt").unwrap();
        line("binding g crdt share commons ttl shape-profile=text_crdt ttl=1h").unwrap();
    }

    /// The tail is parse data in the contract's units, keyed by the line's
    /// glade id: `ttl=` in milliseconds (`Retention.ttl_ms`), the profile by
    /// its GDL-041 name (`ShapeProfileDecl.profile`). A line without a tail
    /// has no entry.
    #[test]
    fn the_tail_is_parse_data_in_the_contracts_units() {
        let decl = parse(
            "glade-app v0\napp x\n\
             binding a value share commons latest\n\
             binding b value share commons ttl ttl=500ms\n\
             binding c log   share commons ttl ttl=30s\n\
             binding d value share commons ttl ttl=10m\n\
             binding e swmr  share commons ttl ttl=1h shape-profile=snapshot_delta\n\
             binding f crdt  share commons ttl shape-profile=text_crdt ttl=7d\n\
             binding g swmr  share commons from-cursor\n\
             binding h value share commons ttl ttl=010s\n",
        )
        .unwrap();
        let tail = |id: &str, ttl_ms: Option<i64>, profile: Option<&str>| BindingTail {
            glade_id: id.into(),
            ttl_ms,
            shape_profile: profile.map(str::to_string),
        };
        assert_eq!(
            decl.tails,
            vec![
                tail("b", Some(500), None),
                tail("c", Some(30_000), None),
                tail("d", Some(600_000), None),
                tail("e", Some(3_600_000), Some("snapshot_delta")),
                tail("f", Some(604_800_000), Some("text_crdt")),
                tail("h", Some(10_000), None),
            ]
        );
        assert_eq!(decl.bindings.len(), 8, "a tail changes no binding count");
    }

    /// An unknown key is refused by name, with its line number — including
    /// `profile`, the name R11's naming note keeps out of the file.
    #[test]
    fn an_unknown_key_is_refused_by_name_with_its_line() {
        assert_eq!(
            parse("glade-app v0\napp x\n\nbinding g value share commons latest colour=red\n").unwrap_err(),
            "line 4: unknown binding key `colour` (binding keys: `ttl`, `shape-profile`)"
        );
        assert_eq!(
            line("binding g swmr share commons from-cursor profile=snapshot_delta").unwrap_err(),
            "line 3: unknown binding key `profile` (binding keys: `ttl`, `shape-profile`)"
        );
        assert_eq!(
            line("binding g value share commons latest TTL=10m").unwrap_err(),
            "line 3: unknown binding key `TTL` (binding keys: `ttl`, `shape-profile`)"
        );
    }

    /// A sixth token that is not `key=value` is refused by name.
    #[test]
    fn a_token_that_is_not_key_value_is_refused() {
        for tok in ["extra", "=10m"] {
            assert_eq!(
                line(&format!("binding g value share commons latest {tok}")).unwrap_err(),
                format!("line 3: `{tok}` is not a key=value entry (binding keys: `ttl`, `shape-profile`)")
            );
        }
    }

    /// A missing positional token cannot hide behind the tail: a `key=value`
    /// entry standing where the zone or the retention goes is refused, naming
    /// the slot, so "there is no default" stays true on a line with a tail.
    #[test]
    fn a_tail_entry_where_a_token_goes_is_refused() {
        assert_eq!(
            line("binding g value share commons ttl=10m").unwrap_err(),
            "line 3: `ttl=10m` is a key=value entry where <retention> goes (the tail follows all five tokens)"
        );
        assert_eq!(
            line("binding g value share latest ttl=10m").unwrap_err(),
            "line 3: `ttl=10m` is a key=value entry where <retention> goes (the tail follows all five tokens)"
        );
        assert_eq!(
            line("binding g swmr share shape-profile=snapshot_delta from-cursor").unwrap_err(),
            "line 3: `shape-profile=snapshot_delta` is a key=value entry where <zone> goes (the tail follows all five tokens)"
        );
    }

    /// A key may appear once per line, even with the same value.
    #[test]
    fn a_repeated_key_is_refused() {
        assert_eq!(
            line("binding g value share commons ttl ttl=1m ttl=1m").unwrap_err(),
            "line 3: repeated binding key `ttl`"
        );
        assert_eq!(
            line("binding g swmr share commons from-cursor shape-profile=snapshot_delta shape-profile=snapshot_delta")
                .unwrap_err(),
            "line 3: repeated binding key `shape-profile`"
        );
    }

    /// `ttl=<duration>`: a whole number above zero and exactly one unit of
    /// `ms`, `s`, `m`, `h` or `d` — nothing else.
    #[test]
    fn a_bad_duration_is_refused() {
        for bad in ["", "10", "m", "0m", "00s", "1.5h", "1h30m", "10M", "-1m", "+1m", "1w", "10mss", "1 m"] {
            let text = format!("binding g value share commons ttl ttl={bad}");
            // A space ends the token, so `ttl=1 m` offers the duration `1`.
            let tok = bad.split_whitespace().next().unwrap_or("");
            assert_eq!(
                line(&text).unwrap_err(),
                format!(
                    "line 3: bad duration `{tok}` (`ttl=` takes a whole number above zero and one unit of ms, s, m, h or d, such as `ttl=10m`)"
                ),
                "ttl={bad}"
            );
        }
        for huge in ["999999999999d", "99999999999999999999ms"] {
            assert_eq!(
                line(&format!("binding g value share commons ttl ttl={huge}")).unwrap_err(),
                format!("line 3: duration `{huge}` is out of range")
            );
        }
    }

    /// `ttl=` goes with the retention `ttl` and no other. A bare `ttl` still
    /// parses, as it did before the tail: it names no duration.
    #[test]
    fn ttl_needs_the_retention_ttl() {
        for retention in ["latest", "from-cursor", "windowed"] {
            assert_eq!(
                line(&format!("binding g log share commons {retention} ttl=10m")).unwrap_err(),
                format!("line 3: `ttl=` needs the retention `ttl` (this line's is `{retention}`)")
            );
        }
        line("binding g value share commons ttl").unwrap();
    }

    /// Exactly GDL-041's profiles, each over its core: `value` and `log`
    /// take none, `swmr` may omit it (the bare engine) and takes only
    /// `snapshot_delta`, `crdt` takes only `text_crdt`.
    #[test]
    fn a_profile_must_fit_its_shape() {
        let profiles = "(profiles: `snapshot_delta` over `swmr`, `text_crdt` over `crdt`)";
        for shape in ["value", "log"] {
            assert_eq!(
                line(&format!("binding g {shape} share commons latest shape-profile=snapshot_delta")).unwrap_err(),
                format!("line 3: shape `{shape}` takes no `shape-profile` {profiles}")
            );
        }
        assert_eq!(
            line("binding g swmr share commons from-cursor shape-profile=text_crdt").unwrap_err(),
            "line 3: profile `text_crdt` is over `crdt`, not `swmr`"
        );
        assert_eq!(
            line("binding g crdt share commons from-cursor shape-profile=snapshot_delta").unwrap_err(),
            "line 3: profile `snapshot_delta` is over `swmr`, not `crdt`"
        );
        for unknown in ["text-crdt", "blob", ""] {
            assert_eq!(
                line(&format!("binding g swmr share commons from-cursor shape-profile={unknown}")).unwrap_err(),
                format!("line 3: unknown profile `{unknown}` {profiles}")
            );
        }
        line("binding g swmr share commons from-cursor").unwrap();
    }

    /// `crdt` without its profile is refused at parse: glial throws at mount
    /// without one, and a file that loads into an unmountable surface is the
    /// defect (§4.4 bullet 4).
    #[test]
    fn crdt_without_a_profile_is_refused() {
        assert_eq!(
            line("binding doc.body crdt share commons from-cursor").unwrap_err(),
            "line 3: shape `crdt` needs `shape-profile=text_crdt` (glial cannot mount a crdt surface without its profile)"
        );
        assert_eq!(
            line("binding doc.body crdt share commons ttl ttl=1d").unwrap_err(),
            "line 3: shape `crdt` needs `shape-profile=text_crdt` (glial cannot mount a crdt surface without its profile)"
        );
    }
}
