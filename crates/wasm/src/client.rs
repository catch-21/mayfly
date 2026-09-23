//! The exported `ChainClient`: one party's client for one chain, over a JS store and signer.
//!
//! Hashes cross the boundary as unpadded base64url strings — the spelling every view uses —
//! and are accepted in any of the three spellings of §6. Everything structured comes back as
//! plain objects built from [`pubky_mayfly_client::view`].
//!
//! The client sits in a `RefCell` and every async method holds the borrow across its awaits
//! on purpose: wasm is single-threaded, and the held borrow is what turns a second concurrent
//! call into a clean `Busy` error instead of two interleaved syncs.

#![allow(clippy::await_holding_refcell_ref)]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use serde_json::Value;
use wasm_bindgen::prelude::*;

use pubky_mayfly::hash::{ChainId, Hash};
use pubky_mayfly::record::CloseReason;
use pubky_mayfly::rules::PartyIndex;
use pubky_mayfly_client::chain::verify_from;
use pubky_mayfly_client::layout::parse_chain_url;
use pubky_mayfly_client::view::{chain_view, ActionView};
use pubky_mayfly_client::{GenesisSpec, Policy};

use crate::error::{input, js, named};
use crate::rules::AnyRules;
use crate::signer::JsSigner;
use crate::store::JsStore;

type Inner = pubky_mayfly_client::ChainClient<AnyRules, JsStore, JsSigner>;

/// Rules by id, or an `InvalidInput` error naming what is accepted.
fn rules(id: &str) -> Result<AnyRules, JsValue> {
    AnyRules::by_id(id).ok_or_else(|| {
        input(format!(
            "unknown rules {id:?}; this build knows {}",
            AnyRules::IDS.join(", ")
        ))
    })
}

fn hash(s: &str) -> Result<Hash, JsValue> {
    Hash::parse(s).map_err(|e| input(format!("hash: {e}")))
}

fn to_js<T: serde::Serialize>(v: &T) -> Result<JsValue, JsValue> {
    let ser = serde_wasm_bindgen::Serializer::json_compatible();
    v.serialize(&ser)
        .map_err(|e| named("Internal", &e.to_string()))
}

fn from_js<T: serde::de::DeserializeOwned>(v: JsValue, what: &str) -> Result<T, JsValue> {
    serde_wasm_bindgen::from_value(v).map_err(|e| input(format!("{what}: {e}")))
}

/// What `create` takes: the parties and the safety parameters of §6.6.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SpecInput {
    /// Every party's pubky, the initiator first.
    parties: Vec<String>,
    /// Per party, the `client_id` they were invited through (folder hint); may be shorter.
    #[serde(default)]
    apps: Vec<String>,
    /// `None` for unanimity.
    #[serde(default)]
    confirm_quorum: Option<u32>,
    #[serde(default)]
    witnesses: Vec<String>,
    #[serde(default)]
    recovery_delay_ms: Option<u64>,
    #[serde(default)]
    max_body_bytes: Option<u64>,
    #[serde(default)]
    options: Option<Value>,
}

/// One party's client for one chain (§8).
///
/// Every method that touches storage is `async`; call them one at a time — a second call while
/// one is in flight throws `Busy`.
#[wasm_bindgen]
pub struct ChainClient {
    inner: Rc<RefCell<Inner>>,
}

impl ChainClient {
    fn wrap(inner: Inner) -> Self {
        Self {
            inner: Rc::new(RefCell::new(inner)),
        }
    }

    fn borrow(&self) -> Result<std::cell::RefMut<'_, Inner>, JsValue> {
        self.inner
            .try_borrow_mut()
            .map_err(|_| named("Busy", "another call on this client is in flight"))
    }
}

#[wasm_bindgen]
impl ChainClient {
    /// Write genesis as `spec.parties[0]` and return the initiator's client (§8.1).
    ///
    /// `spec`: `{ parties, apps?, confirmQuorum?, witnesses?, recoveryDelayMs?, maxBodyBytes?,
    /// options? }`.
    pub async fn create(
        rules_id: String,
        store: JsValue,
        signer: JsValue,
        spec: JsValue,
    ) -> Result<ChainClient, JsValue> {
        let rules = rules(&rules_id)?;
        let store = JsStore::new(store)?;
        let signer = JsSigner::new(signer)?;
        let input: SpecInput = from_js(spec, "spec")?;
        let apps: Vec<&str> = input.apps.iter().map(String::as_str).collect();
        let mut spec = GenesisSpec::new(input.parties);
        if !apps.is_empty() {
            spec = spec.with_apps(&apps);
        }
        spec.confirm_quorum = input.confirm_quorum;
        spec.witnesses = input.witnesses;
        if let Some(d) = input.recovery_delay_ms {
            spec.recovery_delay_ms = d;
        }
        if let Some(m) = input.max_body_bytes {
            spec.max_body_bytes = m;
        }
        if let Some(o) = input.options {
            spec.options = o;
        }
        let inner = Inner::create(rules, store, signer, spec)
            .await
            .map_err(js)?;
        Ok(Self::wrap(inner))
    }

    /// A client for an existing chain from its URL
    /// (`pubky://<owner>/pub/<client_id>/mayfly/chains/<id>/`). Reads nothing until `sync`.
    #[wasm_bindgen(js_name = "openUrl")]
    pub fn open_url(
        rules_id: String,
        store: JsValue,
        signer: JsValue,
        url: String,
    ) -> Result<ChainClient, JsValue> {
        let rules = rules(&rules_id)?;
        let store = JsStore::new(store)?;
        let signer = JsSigner::new(signer)?;
        let inner = Inner::open_url(rules, store, signer, &url).map_err(js)?;
        Ok(Self::wrap(inner))
    }

    /// The invite URL for this chain.
    #[wasm_bindgen(js_name = "inviteUrl")]
    pub fn invite_url(&self) -> Result<String, JsValue> {
        Ok(self.borrow()?.invite_url())
    }

    /// The chain id.
    pub fn chain(&self) -> Result<String, JsValue> {
        Ok(self.borrow()?.chain().to_string())
    }

    /// Every folder this client reads: `[{ owner, path }]`.
    pub fn folders(&self) -> Result<JsValue, JsValue> {
        let f: Vec<Value> = self
            .borrow()?
            .folders()
            .into_iter()
            .map(|(owner, path)| serde_json::json!({ "owner": owner, "path": path }))
            .collect();
        to_js(&f)
    }

    /// Tell the client where else to look: a party's folder learned out of band.
    #[wasm_bindgen(js_name = "addFolder")]
    pub fn add_folder(&self, owner: String, path: String) -> Result<(), JsValue> {
        self.borrow()?.add_folder(owner, path);
        Ok(())
    }

    /// Replace the clock `ts` and skips are judged on: a function returning Unix milliseconds.
    /// The default is `Date.now()`.
    #[wasm_bindgen(js_name = "setClock")]
    pub fn set_clock(&self, clock: js_sys::Function) -> Result<(), JsValue> {
        self.borrow()?.set_clock(move || {
            clock
                .call0(&JsValue::NULL)
                .ok()
                .and_then(|v| v.as_f64())
                .map(|ms| ms.max(0.0) as u64)
                .unwrap_or(0)
        });
        Ok(())
    }

    /// Client policy: `{ awaitWitnesses?, pollMs?, maxFilesPerSync? }`.
    #[wasm_bindgen(js_name = "setPolicy")]
    pub fn set_policy(&self, policy: JsValue) -> Result<(), JsValue> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct P {
            #[serde(default)]
            await_witnesses: Option<usize>,
            #[serde(default)]
            poll_ms: Option<u64>,
            #[serde(default)]
            max_files_per_sync: Option<usize>,
        }
        let p: P = from_js(policy, "policy")?;
        let mut c = self.borrow()?;
        let mut next: Policy = c.policy;
        if let Some(w) = p.await_witnesses {
            next.await_witnesses = w;
        }
        if let Some(ms) = p.poll_ms {
            next.poll = Duration::from_millis(ms);
        }
        if let Some(m) = p.max_files_per_sync {
            next.max_files_per_sync = m;
        }
        c.policy = next;
        Ok(())
    }

    /// Read every known folder and fold. Returns the chain view.
    pub async fn sync(&self) -> Result<JsValue, JsValue> {
        let mut c = self.borrow()?;
        let report = c.sync().await.map_err(js)?;
        to_js(&chain_view(c.rules(), &report.verdict, &report.suspects))
    }

    /// Confirm genesis with my Grant and folder (§8.1). Returns the confirmation's hash.
    pub async fn join(&self) -> Result<String, JsValue> {
        Ok(self.borrow()?.join().await.map_err(js)?.to_base64url())
    }

    /// Sync, mirror, and take the honest step (§8.2). Returns the actions taken or needed.
    pub async fn act(&self) -> Result<JsValue, JsValue> {
        let actions = self.borrow()?.act().await.map_err(js)?;
        let views: Vec<ActionView> = actions.iter().map(ActionView::from).collect();
        to_js(&views)
    }

    /// Propose rules content whose `kind` is inside `body` (e.g. `{ kind: "add", id, text }`).
    #[wasm_bindgen(js_name = "proposeBody")]
    pub async fn propose_body(&self, body: JsValue) -> Result<String, JsValue> {
        let body: Value = from_js(body, "body")?;
        Ok(self
            .borrow()?
            .propose_body(&body)
            .await
            .map_err(js)?
            .to_base64url())
    }

    /// Propose rules content of `kind` with `body`.
    pub async fn propose(&self, kind: String, body: JsValue) -> Result<String, JsValue> {
        let body: Value = from_js(body, "body")?;
        Ok(self
            .borrow()?
            .propose(&kind, body)
            .await
            .map_err(js)?
            .to_base64url())
    }

    /// Vote for a candidate at the open seq.
    pub async fn confirm(&self, link: String) -> Result<String, JsValue> {
        let h = hash(&link)?;
        Ok(self.borrow()?.confirm(h).await.map_err(js)?.to_base64url())
    }

    /// Vote against a candidate at the open seq.
    pub async fn reject(&self, link: String) -> Result<String, JsValue> {
        let h = hash(&link)?;
        Ok(self.borrow()?.reject(h).await.map_err(js)?.to_base64url())
    }

    /// As designated proposer after a dead round, re-propose `earlier`'s content.
    pub async fn repropose(&self, earlier: String) -> Result<String, JsValue> {
        let h = hash(&earlier)?;
        Ok(self
            .borrow()?
            .repropose(h)
            .await
            .map_err(js)?
            .to_base64url())
    }

    /// As designated proposer, pass.
    pub async fn pass(&self) -> Result<String, JsValue> {
        Ok(self.borrow()?.pass().await.map_err(js)?.to_base64url())
    }

    /// Skip a silent designated proposer.
    pub async fn skip(&self) -> Result<String, JsValue> {
        Ok(self.borrow()?.skip().await.map_err(js)?.to_base64url())
    }

    /// What the designated proposer carries forward from a dead round, if anything.
    #[wasm_bindgen(js_name = "lowestVotedInDeadRound")]
    pub fn lowest_voted_in_dead_round(&self) -> Result<Option<String>, JsValue> {
        Ok(self
            .borrow()?
            .lowest_voted_in_dead_round()
            .map_err(js)?
            .map(|h| h.to_base64url()))
    }

    /// Propose an ordinary close: `"agreed"` or `"finished"`.
    #[wasm_bindgen(js_name = "proposeClose")]
    pub async fn propose_close(&self, reason: String) -> Result<String, JsValue> {
        let reason = match reason.as_str() {
            "agreed" => CloseReason::Agreed,
            "finished" => CloseReason::Finished,
            other => {
                return Err(input(format!(
                    "close reason {other:?}: use \"agreed\" or \"finished\" (abandoned has its own call)"
                )))
            }
        };
        Ok(self
            .borrow()?
            .propose_close(reason)
            .await
            .map_err(js)?
            .to_base64url())
    }

    /// Propose closing on silent parties, by index.
    #[wasm_bindgen(js_name = "proposeAbandoned")]
    pub async fn propose_abandoned(&self, subjects: Vec<u32>) -> Result<String, JsValue> {
        let subjects: Vec<PartyIndex> = subjects.into_iter().map(|s| s as PartyIndex).collect();
        Ok(self
            .borrow()?
            .propose_abandoned(&subjects)
            .await
            .map_err(js)?
            .to_base64url())
    }

    /// Confirm an abandoned close.
    #[wasm_bindgen(js_name = "confirmAbandoned")]
    pub async fn confirm_abandoned(&self, close: String) -> Result<String, JsValue> {
        let h = hash(&close)?;
        Ok(self
            .borrow()?
            .confirm_abandoned(h)
            .await
            .map_err(js)?
            .to_base64url())
    }

    /// Reveal my commit-reveal nonce, when the rules want reveals.
    #[wasm_bindgen(js_name = "proposeReveal")]
    pub async fn propose_reveal(&self) -> Result<String, JsValue> {
        Ok(self
            .borrow()?
            .propose_reveal()
            .await
            .map_err(js)?
            .to_base64url())
    }

    /// Copy committed links, QCs, receipts and engagements into my folder (§7). Returns how
    /// many files were written. `act` does this itself.
    pub async fn mirror(&self) -> Result<usize, JsValue> {
        self.borrow()?.mirror().await.map_err(js)
    }

    /// Poll the known folders until a listing changes or `timeoutMs` passes.
    #[wasm_bindgen(js_name = "waitForChange")]
    pub async fn wait_for_change(&self, timeout_ms: u32) -> Result<bool, JsValue> {
        self.borrow()?
            .wait_for_change(Duration::from_millis(u64::from(timeout_ms)))
            .await
            .map_err(js)
    }

    /// My index in genesis order.
    #[wasm_bindgen(js_name = "myIndex")]
    pub fn my_index(&self) -> Result<u32, JsValue> {
        Ok(self.borrow()?.my_index().map_err(js)? as u32)
    }

    /// The rules state at the head, or `undefined` before the rules initialise.
    pub fn state(&self) -> Result<JsValue, JsValue> {
        match self.borrow()?.state().map_err(js)? {
            Some(s) => to_js(&s),
            None => Ok(JsValue::UNDEFINED),
        }
    }

    /// The terms genesis names — parties, rules, watchmen, quorum — once a sync has found the
    /// genesis link, including before it commits. `undefined` before that.
    pub fn arrangement(&self) -> Result<JsValue, JsValue> {
        match self.borrow()?.arrangement() {
            Some(a) => to_js(&a),
            None => Ok(JsValue::UNDEFINED),
        }
    }

    /// The chain as of the last sync, or `undefined` before one.
    pub fn view(&self) -> Result<JsValue, JsValue> {
        let c = self.borrow()?;
        match c.verdict() {
            Some(v) => to_js(&chain_view(c.rules(), v, &[])),
            None => Ok(JsValue::UNDEFINED),
        }
    }
}

/// Verify a chain from files alone, as anyone: no seat, no signer. `store.me` may be `""`.
#[wasm_bindgen(js_name = "verifyFrom")]
pub async fn verify_from_url(
    rules_id: String,
    store: JsValue,
    chain_url: String,
) -> Result<JsValue, JsValue> {
    let rules = rules(&rules_id)?;
    let store = JsStore::new(store)?;
    let (chain, initiator) = parse_chain_url(&chain_url).map_err(js)?;
    let report = verify_from(&rules, &store, &chain, initiator)
        .await
        .map_err(js)?;
    to_js(&chain_view(&rules, &report.verdict, &report.suspects))
}

/// Decode one record file for inspection (§14): header, payload with embedded records
/// unpacked, signature under the claimed key, bytes against the file name and `etag`.
#[wasm_bindgen(js_name = "decodeRecord")]
pub fn decode_record(bytes: &[u8], name: String, etag: Option<String>) -> Result<JsValue, JsValue> {
    let etag = match etag {
        Some(e) => Some(hash(&e)?),
        None => None,
    };
    to_js(&pubky_mayfly_client::view::decode_record(
        bytes,
        &name,
        etag.as_ref(),
    ))
}

/// Parse a chain URL into `{ chain, owner, folder }`.
#[wasm_bindgen(js_name = "parseChainUrl")]
pub fn parse_chain_url_js(url: String) -> Result<JsValue, JsValue> {
    let (chain, (owner, folder)) = parse_chain_url(&url).map_err(js)?;
    to_js(&serde_json::json!({
        "chain": chain.to_string(),
        "owner": owner,
        "folder": folder,
    }))
}

/// The chain URL for `chain` under `owner`'s protocol folder.
#[wasm_bindgen(js_name = "chainUrl")]
pub fn chain_url_js(owner: String, folder: String, chain: String) -> Result<String, JsValue> {
    let id = ChainId::parse(&chain).map_err(|e| input(e.to_string()))?;
    let folder = pubky_mayfly_client::layout::Folder::from_path(&folder);
    Ok(format!("pubky://{owner}{}", folder.chain(&id).as_str()))
}

/// The rules ids this build knows.
#[wasm_bindgen(js_name = "rulesIds")]
pub fn rules_ids() -> Vec<String> {
    AnyRules::IDS.iter().map(|s| s.to_string()).collect()
}

/// Whose turn a round `>= 1` at `seq` is, among `parties` (§6.4 rotation) — for a page that
/// shows who the chain is waiting on.
#[wasm_bindgen(js_name = "designatedProposer")]
pub fn designated_proposer(
    chain: String,
    seq: f64,
    round: u32,
    parties: u32,
) -> Result<u32, JsValue> {
    if round == 0 {
        return Err(input("round 0 has no designated proposer"));
    }
    if seq < 0.0 || seq.fract() != 0.0 {
        return Err(input("seq must be a non-negative integer"));
    }
    let id = ChainId::parse(&chain).map_err(|e| input(e.to_string()))?;
    Ok(pubky_mayfly::vote::designated(&id, seq as u64, round, parties as usize) as u32)
}
