//! `list/1` — a shared list (§10.1).
//!
//! Any party may append. Nobody is *obliged* to, so the stopwatch charges only responses. The
//! rules are deliberately trivial so that the first end-to-end tests exercise the protocol, not
//! the application.

use serde::{Deserialize, Serialize};

use pubky_mayfly::genesis::Genesis;
use pubky_mayfly::record::{Confirmation, Link};
use pubky_mayfly::rules::{CloseBody, Nonce, Outcome, PartyIndex, Rules, RulesError, Status};

/// The rules id.
pub const ID: &str = "list/1";

/// One list item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// Stable id chosen by the adder.
    pub id: String,
    /// Text.
    pub text: String,
    /// Optional quantity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qty: Option<u64>,
    /// Ticked off.
    pub ticked: bool,
}

/// List state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// Items in insertion order.
    pub items: Vec<Item>,
    /// Archived lists are terminal.
    pub archived: bool,
    /// Number of parties, for `may_append`.
    pub parties: usize,
}

/// Link body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    /// Add an item.
    Add {
        /// Id.
        id: String,
        /// Text.
        text: String,
        /// Quantity.
        #[serde(default)]
        qty: Option<u64>,
    },
    /// Edit text or quantity.
    Edit {
        /// Id.
        id: String,
        /// New text.
        #[serde(default)]
        text: Option<String>,
        /// New quantity.
        #[serde(default)]
        qty: Option<u64>,
    },
    /// Tick.
    Tick {
        /// Id.
        id: String,
    },
    /// Untick.
    Untick {
        /// Id.
        id: String,
    },
    /// Remove.
    Remove {
        /// Id.
        id: String,
    },
    /// Archive: terminal.
    Archive {},
}

/// The `list/1` rules.
#[derive(Debug, Default, Clone, Copy)]
pub struct List;

impl Rules for List {
    type State = State;
    type Body = Body;

    fn id(&self) -> &'static str {
        ID
    }

    fn reference_hash(&self) -> &'static str {
        // Replaced by the BLAKE3 of the published WASM module at release (§6.6).
        "list/1-reference-hash-placeholder"
    }

    fn init(
        &self,
        genesis: &Genesis,
        _confirmations: &[Confirmation],
        _nonces: &[Nonce],
    ) -> Result<State, RulesError> {
        Ok(State {
            parties: genesis.parties.len(),
            ..State::default()
        })
    }

    fn wants_reveals(&self, _genesis: &Genesis) -> bool {
        false
    }

    fn obliged(&self, _state: &State) -> Vec<PartyIndex> {
        Vec::new()
    }

    fn may_append(&self, state: &State, party: PartyIndex, _kind: &str) -> bool {
        !state.archived && party < state.parties
    }

    fn apply(&self, state: &State, link: &Link) -> Result<State, RulesError> {
        if state.archived {
            return Err(RulesError("list is archived".into()));
        }
        let mut body = link.body.clone();
        if let Some(obj) = body.as_object_mut() {
            obj.insert("kind".into(), serde_json::Value::String(link.kind.clone()));
        }
        let body: Body =
            serde_json::from_value(body).map_err(|e| RulesError(format!("body: {e}")))?;
        let mut next = state.clone();
        let find = |items: &mut Vec<Item>, id: &str| -> Result<usize, RulesError> {
            items
                .iter()
                .position(|i| i.id == id)
                .ok_or_else(|| RulesError(format!("no item {id}")))
        };
        match body {
            Body::Add { id, text, qty } => {
                if next.items.iter().any(|i| i.id == id) {
                    return Err(RulesError(format!("duplicate item {id}")));
                }
                next.items.push(Item {
                    id,
                    text,
                    qty,
                    ticked: false,
                });
            }
            Body::Edit { id, text, qty } => {
                let i = find(&mut next.items, &id)?;
                if let Some(t) = text {
                    next.items[i].text = t;
                }
                if qty.is_some() {
                    next.items[i].qty = qty;
                }
            }
            Body::Tick { id } => {
                let i = find(&mut next.items, &id)?;
                next.items[i].ticked = true;
            }
            Body::Untick { id } => {
                let i = find(&mut next.items, &id)?;
                next.items[i].ticked = false;
            }
            Body::Remove { id } => {
                let i = find(&mut next.items, &id)?;
                next.items.remove(i);
            }
            Body::Archive {} => next.archived = true,
        }
        Ok(next)
    }

    fn status(&self, state: &State) -> Status {
        if state.archived {
            Status::Finished(Outcome {
                summary: "archived".into(),
                winners: Vec::new(),
            })
        } else {
            Status::Ongoing
        }
    }

    fn close(&self, state: &State, close: &CloseBody) -> Result<Outcome, RulesError> {
        use pubky_mayfly::record::CloseReason::*;
        match close.reason {
            Finished if !state.archived => Err(RulesError("list is not archived".into())),
            Finished | Agreed | Abandoned => Ok(Outcome {
                summary: "archived".into(),
                winners: Vec::new(),
            }),
        }
    }

    fn canonical_state(&self, state: &State) -> Vec<u8> {
        // Field order is fixed by the struct; serde_json with preserve_order keeps it.
        serde_json::to_vec(state).expect("state serialises")
    }
}
