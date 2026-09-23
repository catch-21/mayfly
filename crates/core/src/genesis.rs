//! Genesis body and the safety-parameter checks a confirmer applies (§6.6).
//!
//! A genesis confirmation is consent to the fault model. Anything a confirmer would not accept
//! it has to reject at genesis; nothing here is renegotiated later.

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// Default `think_ms` / `respond_ms` when `time_control` is absent: 24 hours (§11.3).
pub const DEFAULT_ALLOWANCE_MS: u64 = 86_400_000;

/// Floor for `recovery_delay_ms` (§6.6).
pub const MIN_RECOVERY_DELAY_MS: u64 = 86_400_000;

/// Default cap a client is willing to fetch per record (§6.6).
pub const DEFAULT_MAX_BODY_BYTES: u64 = 1_048_576;

/// The `body` of link 0.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Genesis {
    /// Rules id, e.g. `chess/1`.
    pub rules: String,
    /// BLAKE3 of the reference rules module for `rules`.
    pub rules_hash: String,
    /// Largest record any party will write or any verifier will read.
    pub max_body_bytes: u64,
    /// The parties, in the order used for rotation and reveals. The initiator is first and is
    /// the only one whose `kid` is known here; the others arrive in their confirmations.
    pub parties: Vec<Party>,
    /// Initiator's domain-separating nonce (public from the start; adds no randomness).
    pub nonce: String,
    /// Votes, including the author's, needed to commit. `N/2 < q <= N`; default `N`.
    pub confirm_quorum: u32,
    /// Witnesses, by pubky.
    #[serde(default)]
    pub witnesses: Vec<Witness>,
    /// How long a `recover` must wait before counterparties confirm it.
    pub recovery_delay_ms: u64,
    /// Rules options; `time_control` is read by the protocol.
    #[serde(default)]
    pub options: serde_json::Value,
}

/// One party as named at genesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Party {
    /// Identity (z32).
    pub pubky: String,
    /// Chain key (z32); present for the initiator only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kid: Option<String>,
    /// Rules-defined role, e.g. `white`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Discovery hint: where the initiator expects this party's records (`/pub/<app>/mayfly/`),
    /// so their genesis confirmation can be found before link 1 embeds it (§9.1). The `path`
    /// in the party's own confirmation is authoritative; this is never checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// One witness as named at genesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Witness {
    /// Identity (z32).
    pub pubky: String,
}

/// `options.time_control`, with the §11.3 defaults applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeControl {
    /// Allowance for the obliged party to propose.
    pub think_ms: u64,
    /// Allowance for a confirmer to respond to a proposal.
    pub respond_ms: u64,
}

impl Genesis {
    /// Number of parties.
    pub fn n(&self) -> u32 {
        self.parties.len() as u32
    }

    /// Time control with defaults.
    pub fn time_control(&self) -> TimeControl {
        let tc = self.options.get("time_control");
        let get = |k: &str| {
            tc.and_then(|t| t.get(k))
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_ALLOWANCE_MS)
        };
        TimeControl {
            think_ms: get("think_ms"),
            respond_ms: get("respond_ms"),
        }
    }

    /// The safety-parameter checks of §6.6. A confirmer refuses, and a verifier treats the
    /// genesis as invalid — the chain never existed — if any fails.
    ///
    /// `known_rules` returns the reference `rules_hash` for a rules id this implementation
    /// runs, or `None` if it does not know the id. `max_body_cap` is what this client is
    /// willing to fetch.
    pub fn check_safety(
        &self,
        known_rules: impl Fn(&str) -> Option<String>,
        max_body_cap: u64,
    ) -> Result<(), Error> {
        let n = self.n();
        if n < 2 {
            return Err(Error::Genesis("fewer than two parties".into()));
        }
        let mut seen = std::collections::HashSet::new();
        for p in &self.parties {
            if crate::keys::parse_z32(&p.pubky).is_err() {
                return Err(Error::Genesis(format!(
                    "party {:?} is not a pubky",
                    p.pubky
                )));
            }
            if !seen.insert(&p.pubky) {
                return Err(Error::Genesis(format!("duplicate party {}", p.pubky)));
            }
        }
        for w in &self.witnesses {
            if crate::keys::parse_z32(&w.pubky).is_err() {
                return Err(Error::Genesis(format!(
                    "witness {:?} is not a pubky",
                    w.pubky
                )));
            }
        }
        if self.parties[0].kid.is_none() {
            return Err(Error::Genesis("initiator kid missing".into()));
        }
        let q = self.confirm_quorum;
        if q == 0 || q > n || 2 * q <= n {
            return Err(Error::Genesis(format!(
                "confirm_quorum {q} not in N/2 < q <= N for N = {n}"
            )));
        }
        match known_rules(&self.rules) {
            None => return Err(Error::Genesis(format!("unknown rules {}", self.rules))),
            Some(h) if h != self.rules_hash => {
                return Err(Error::Genesis("rules_hash does not match reference".into()))
            }
            Some(_) => {}
        }
        if self.max_body_bytes == 0 || self.max_body_bytes > max_body_cap {
            return Err(Error::Genesis(format!(
                "max_body_bytes {} outside 1..={max_body_cap}",
                self.max_body_bytes
            )));
        }
        if self.recovery_delay_ms < MIN_RECOVERY_DELAY_MS {
            return Err(Error::Genesis("recovery_delay_ms below floor".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn z32() -> String {
        pubky_common::crypto::Keypair::random().public_key().z32()
    }

    fn party(pubky: String, kid: Option<String>) -> Party {
        Party {
            pubky,
            kid,
            role: None,
            path: None,
        }
    }

    /// A genesis every safety check accepts, so each refusal below is one change.
    fn acceptable() -> Genesis {
        Genesis {
            rules: "list/1".into(),
            rules_hash: "list/1-hash".into(),
            max_body_bytes: 65_536,
            parties: vec![party(z32(), Some(z32())), party(z32(), None)],
            nonce: "n".into(),
            confirm_quorum: 2,
            witnesses: Vec::new(),
            recovery_delay_ms: MIN_RECOVERY_DELAY_MS,
            options: serde_json::Value::Null,
        }
    }

    fn know(id: &str) -> Option<String> {
        (id == "list/1").then(|| "list/1-hash".into())
    }

    fn refuse(g: &Genesis) -> String {
        g.check_safety(know, DEFAULT_MAX_BODY_BYTES)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn check_safety_refuses_an_unsafe_genesis() {
        acceptable()
            .check_safety(know, DEFAULT_MAX_BODY_BYTES)
            .unwrap();

        let mut g = acceptable();
        g.parties.truncate(1);
        assert!(refuse(&g).contains("fewer than two parties"));

        let mut g = acceptable();
        g.parties[1].pubky = "not-a-pubky".into();
        assert!(refuse(&g).contains("is not a pubky"));

        let mut g = acceptable();
        g.parties[1].pubky = g.parties[0].pubky.clone();
        assert!(refuse(&g).contains("duplicate party"));

        let mut g = acceptable();
        g.witnesses.push(Witness {
            pubky: "not-a-pubky".into(),
        });
        assert!(refuse(&g).contains("witness"));
        assert!(refuse(&g).contains("is not a pubky"));

        let mut g = acceptable();
        g.parties[0].kid = None;
        assert!(refuse(&g).contains("initiator kid missing"));

        let mut g = acceptable();
        g.confirm_quorum = 1;
        assert!(refuse(&g).contains("confirm_quorum"));

        let mut g = acceptable();
        g.rules = "chess/1".into();
        assert!(refuse(&g).contains("unknown rules"));

        let mut g = acceptable();
        g.rules_hash = "other".into();
        assert!(refuse(&g).contains("rules_hash"));

        let mut g = acceptable();
        g.max_body_bytes = 0;
        assert!(refuse(&g).contains("max_body_bytes"));

        let mut g = acceptable();
        g.max_body_bytes = DEFAULT_MAX_BODY_BYTES + 1;
        assert!(refuse(&g).contains("max_body_bytes"));

        let mut g = acceptable();
        g.recovery_delay_ms = MIN_RECOVERY_DELAY_MS - 1;
        assert!(refuse(&g).contains("recovery_delay_ms"));
    }
}
