//! Rules chosen by id at runtime, with JSON state and body, so one exported `ChainClient`
//! type serves every rules module this package ships.

use serde_json::Value;

use pubky_mayfly::genesis::Genesis;
use pubky_mayfly::record::{CloseBody, Confirmation, Link};
use pubky_mayfly::rules::{Nonce, Outcome, PartyIndex, Rules, RulesError, Status};
use pubky_mayfly_rules::list::List;

/// Every rules module this package knows.
#[derive(Debug, Clone)]
pub enum AnyRules {
    /// `list/1`.
    List(List),
}

impl AnyRules {
    /// The rules for a pinned id, if shipped.
    pub fn by_id(id: &str) -> Option<Self> {
        match id {
            "list/1" => Some(Self::List(List)),
            _ => None,
        }
    }

    /// Every id this package accepts.
    pub const IDS: &'static [&'static str] = &["list/1"];
}

fn conv<T: serde::Serialize>(v: T) -> Result<Value, RulesError> {
    serde_json::to_value(v).map_err(|e| RulesError(format!("state: {e}")))
}

fn state<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, RulesError> {
    serde_json::from_value(v.clone()).map_err(|e| RulesError(format!("state: {e}")))
}

impl Rules for AnyRules {
    type State = Value;
    type Body = Value;

    fn id(&self) -> &'static str {
        match self {
            Self::List(r) => r.id(),
        }
    }

    fn reference_hash(&self) -> &'static str {
        match self {
            Self::List(r) => r.reference_hash(),
        }
    }

    fn init(
        &self,
        genesis: &Genesis,
        confirmations: &[Confirmation],
        nonces: &[Nonce],
    ) -> Result<Value, RulesError> {
        match self {
            Self::List(r) => conv(r.init(genesis, confirmations, nonces)?),
        }
    }

    fn wants_reveals(&self, genesis: &Genesis) -> bool {
        match self {
            Self::List(r) => r.wants_reveals(genesis),
        }
    }

    fn obliged(&self, s: &Value) -> Vec<PartyIndex> {
        match self {
            Self::List(r) => state(s).map(|s| r.obliged(&s)).unwrap_or_default(),
        }
    }

    fn may_append(&self, s: &Value, party: PartyIndex, kind: &str) -> bool {
        match self {
            Self::List(r) => state(s)
                .map(|s| r.may_append(&s, party, kind))
                .unwrap_or(false),
        }
    }

    fn apply(&self, s: &Value, link: &Link) -> Result<Value, RulesError> {
        match self {
            Self::List(r) => conv(r.apply(&state(s)?, link)?),
        }
    }

    fn status(&self, s: &Value) -> Status {
        match self {
            Self::List(r) => state(s).map(|s| r.status(&s)).unwrap_or(Status::Ongoing),
        }
    }

    fn close(&self, s: &Value, close: &CloseBody) -> Result<Outcome, RulesError> {
        match self {
            Self::List(r) => r.close(&state(s)?, close),
        }
    }

    fn canonical_state(&self, s: &Value) -> Vec<u8> {
        match self {
            Self::List(r) => state(s).map(|s| r.canonical_state(&s)).unwrap_or_default(),
        }
    }
}
