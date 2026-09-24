//! The exported `ChainClient`: one party's client for one chain, over a JS store and signer;
//! `ChainReader` for anyone; and the free functions a page needs around them.
//!
//! Hashes cross the boundary as unpadded base64url strings — the spelling every view uses —
//! and are accepted in any of the three spellings of §6. Everything structured comes back as
//! plain objects built from [`pubky_mayfly_client::view`].
//!
//! Every call that touches storage takes an async lock, so two calls from the page — a tick
//! of the loop and a click — run one after the other rather than the second failing. The
//! readers (`view`, `session`, `state`, `arrangement`) return a snapshot taken after the last
//! call finished, so a render never waits and never sees a half-updated client.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use futures_util::lock::Mutex;
use serde_json::Value;
use wasm_bindgen::prelude::*;

use pubky_mayfly::hash::{ChainId, Hash};
use pubky_mayfly::record::CloseReason;
use pubky_mayfly::rules::PartyIndex;
use pubky_mayfly_client::chain::verify_from;
use pubky_mayfly_client::layout::parse_chain_url;
use pubky_mayfly_client::reader::Reader;
use pubky_mayfly_client::view::{chain_view, ActionView};
use pubky_mayfly_client::{GenesisSpec, Policy};

use crate::error::{input, js, named};
use crate::rules::AnyRules;
use crate::signer::JsSigner;
use crate::store::JsStore;

type Inner = pubky_mayfly_client::ChainClient<AnyRules, JsStore, JsSigner>;

/// Rules from what the caller passed: a shipped rules id (`"list/1"`), or a rules object
/// (see [`crate::jsrules`]). Anything else is an `InvalidInput` error naming what is
/// accepted.
fn rules(v: JsValue) -> Result<AnyRules, JsValue> {
    if let Some(id) = v.as_string() {
        return AnyRules::by_id(&id).ok_or_else(|| {
            input(format!(
                "unknown rules {id:?}; this build ships {}; pass a rules object for others",
                AnyRules::IDS.join(", ")
            ))
        });
    }
    crate::jsrules::JsRules::new(v).map(AnyRules::Js)
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

/// What the readers return between calls.
#[derive(Default)]
struct Snapshot {
    chain: String,
    invite_url: String,
    view: JsValue,
    session: JsValue,
    state: JsValue,
    arrangement: JsValue,
}

/// One party's client for one chain (§8).
///
/// Methods that touch storage are `async` and run one at a time; a call made while another is
/// in flight waits for it. `view()`, `session()`, `state()` and `arrangement()` are
/// synchronous and read the snapshot taken after the last call.
#[wasm_bindgen]
pub struct ChainClient {
    inner: Rc<Mutex<Inner>>,
    snapshot: Rc<RefCell<Snapshot>>,
}

impl ChainClient {
    fn wrap(inner: Inner) -> Self {
        let this = Self {
            inner: Rc::new(Mutex::new(inner)),
            snapshot: Rc::new(RefCell::new(Snapshot::default())),
        };
        if let Some(c) = this.inner.try_lock() {
            this.snap(&c);
        }
        this
    }

    /// Take the snapshot from a client the caller holds the lock on.
    fn snap(&self, c: &Inner) {
        let view = match c.verdict() {
            Some(v) => to_js(&chain_view(c.rules(), v, &[])).unwrap_or(JsValue::UNDEFINED),
            None => JsValue::UNDEFINED,
        };
        let state = match c.state() {
            Ok(Some(s)) => to_js(&s).unwrap_or(JsValue::UNDEFINED),
            _ => JsValue::UNDEFINED,
        };
        let arrangement = match c.arrangement() {
            Some(a) => to_js(&a).unwrap_or(JsValue::UNDEFINED),
            None => JsValue::UNDEFINED,
        };
        let session = to_js(&c.session_view()).unwrap_or(JsValue::UNDEFINED);
        *self.snapshot.borrow_mut() = Snapshot {
            chain: c.chain().to_string(),
            invite_url: c.invite_url(),
            view,
            session,
            state,
            arrangement,
        };
    }

    /// Run `f` with the lock, then refresh the snapshot whatever the outcome.
    async fn with<T>(
        &self,
        f: impl AsyncFnOnce(&mut Inner) -> Result<T, pubky_mayfly_client::Error>,
    ) -> Result<T, JsValue> {
        let mut c = self.inner.lock().await;
        let out = f(&mut c).await;
        self.snap(&c);
        out.map_err(js)
    }

    /// A synchronous change to the client; `Busy` if a call is in flight.
    fn now<T>(&self, f: impl FnOnce(&mut Inner) -> Result<T, JsValue>) -> Result<T, JsValue> {
        let mut c = self
            .inner
            .try_lock()
            .ok_or_else(|| named("Busy", "another call on this client is in flight"))?;
        let out = f(&mut c);
        self.snap(&c);
        out
    }
}

#[wasm_bindgen]
impl ChainClient {
    /// Write genesis as `spec.parties[0]` and return the initiator's client (§8.1).
    ///
    /// `rules`: a shipped rules id (`"list/1"`) or a rules object. `spec`: `{ parties, apps?,
    /// confirmQuorum?, witnesses?, recoveryDelayMs?, maxBodyBytes?, options? }`.
    pub async fn create(
        rules: JsValue,
        store: JsValue,
        signer: JsValue,
        spec: JsValue,
    ) -> Result<ChainClient, JsValue> {
        let rules = self::rules(rules)?;
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
    /// `rules` is a shipped rules id or a rules object.
    #[wasm_bindgen(js_name = "openUrl")]
    pub fn open_url(
        rules: JsValue,
        store: JsValue,
        signer: JsValue,
        url: String,
    ) -> Result<ChainClient, JsValue> {
        let rules = self::rules(rules)?;
        let store = JsStore::new(store)?;
        let signer = JsSigner::new(signer)?;
        let inner = Inner::open_url(rules, store, signer, &url).map_err(js)?;
        Ok(Self::wrap(inner))
    }

    /// The invite URL for this chain.
    #[wasm_bindgen(js_name = "inviteUrl")]
    pub fn invite_url(&self) -> String {
        self.snapshot.borrow().invite_url.clone()
    }

    /// The chain id.
    pub fn chain(&self) -> String {
        self.snapshot.borrow().chain.clone()
    }

    /// Every folder this client reads: `[{ owner, path }]`.
    pub fn folders(&self) -> Result<JsValue, JsValue> {
        self.now(|c| {
            let f: Vec<Value> = c
                .folders()
                .into_iter()
                .map(|(owner, path)| serde_json::json!({ "owner": owner, "path": path }))
                .collect();
            to_js(&f)
        })
    }

    /// Tell the client where else to look: a party's folder learned out of band.
    #[wasm_bindgen(js_name = "addFolder")]
    pub fn add_folder(&self, owner: String, path: String) -> Result<(), JsValue> {
        self.now(|c| {
            c.add_folder(owner, path);
            Ok(())
        })
    }

    /// Replace the clock `ts` and skips are judged on: a function returning Unix milliseconds.
    /// The default is `Date.now()`.
    #[wasm_bindgen(js_name = "setClock")]
    pub fn set_clock(&self, clock: js_sys::Function) -> Result<(), JsValue> {
        self.now(|c| {
            c.set_clock(move || {
                clock
                    .call0(&JsValue::NULL)
                    .ok()
                    .and_then(|v| v.as_f64())
                    .map(|ms| ms.max(0.0) as u64)
                    .unwrap_or(0)
            });
            Ok(())
        })
    }

    /// Client policy: `{ awaitWitnesses?, pollMs?, maxFilesPerSync?, autoPass? }`.
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
            #[serde(default)]
            auto_pass: Option<bool>,
        }
        let p: P = from_js(policy, "policy")?;
        self.now(|c| {
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
            if let Some(a) = p.auto_pass {
                next.auto_pass = a;
            }
            c.policy = next;
            Ok(())
        })
    }

    /// Read every known folder and fold. Returns the chain view.
    pub async fn sync(&self) -> Result<JsValue, JsValue> {
        self.with(async |c| {
            let report = c.sync().await?;
            Ok(chain_view(c.rules(), &report.verdict, &report.suspects))
        })
        .await
        .and_then(|v| to_js(&v))
    }

    /// Confirm genesis with my Grant and folder (§8.1). Returns the confirmation's hash.
    pub async fn join(&self) -> Result<String, JsValue> {
        self.with(async |c| c.join().await)
            .await
            .map(|h| h.to_base64url())
    }

    /// Sync, mirror, and take the honest step (§8.2); then try the oldest held proposal, and
    /// pass if policy says so. Returns the actions taken or needed.
    pub async fn act(&self) -> Result<JsValue, JsValue> {
        let actions = self.with(async |c| c.act().await).await?;
        let views: Vec<ActionView> = actions.iter().map(ActionView::from).collect();
        to_js(&views)
    }

    /// Propose rules content whose `kind` is inside `body` (e.g. `{ kind: "add", id, text }`).
    #[wasm_bindgen(js_name = "proposeBody")]
    pub async fn propose_body(&self, body: JsValue) -> Result<String, JsValue> {
        let body: Value = from_js(body, "body")?;
        self.with(async |c| c.propose_value(body).await)
            .await
            .map(|h| h.to_base64url())
    }

    /// Propose rules content of `kind` with `body`.
    pub async fn propose(&self, kind: String, body: JsValue) -> Result<String, JsValue> {
        let body: Value = from_js(body, "body")?;
        self.with(async |c| c.propose(&kind, body).await)
            .await
            .map(|h| h.to_base64url())
    }

    /// Hold rules content until a round takes it; `act()` proposes it when it can and reports
    /// `proposed` or `held_refused`. `session().held` lists what is waiting. Queued like every
    /// other call, so a hold made during a tick is not lost.
    pub async fn hold(&self, body: JsValue) -> Result<(), JsValue> {
        let body: Value = from_js(body, "body")?;
        self.with(async |c| c.hold_value(body)).await
    }

    /// Take back the held proposal at `index`; returns it, or `undefined`.
    pub async fn withdraw(&self, index: u32) -> Result<JsValue, JsValue> {
        let taken = self.with(async |c| Ok(c.withdraw(index as usize))).await?;
        match taken {
            Some(v) => to_js(&v),
            None => Ok(JsValue::UNDEFINED),
        }
    }

    /// Drop every held proposal.
    #[wasm_bindgen(js_name = "clearHeld")]
    pub async fn clear_held(&self) -> Result<(), JsValue> {
        self.with(async |c| {
            c.clear_held();
            Ok(())
        })
        .await
    }

    /// Vote for a candidate at the open seq.
    pub async fn confirm(&self, link: String) -> Result<String, JsValue> {
        let h = hash(&link)?;
        self.with(async |c| c.confirm(h).await)
            .await
            .map(|h| h.to_base64url())
    }

    /// Vote against a candidate at the open seq.
    pub async fn reject(&self, link: String) -> Result<String, JsValue> {
        let h = hash(&link)?;
        self.with(async |c| c.reject(h).await)
            .await
            .map(|h| h.to_base64url())
    }

    /// As designated proposer after a dead round, re-propose `earlier`'s content.
    pub async fn repropose(&self, earlier: String) -> Result<String, JsValue> {
        let h = hash(&earlier)?;
        self.with(async |c| c.repropose(h).await)
            .await
            .map(|h| h.to_base64url())
    }

    /// As designated proposer, pass.
    pub async fn pass(&self) -> Result<String, JsValue> {
        self.with(async |c| c.pass().await)
            .await
            .map(|h| h.to_base64url())
    }

    /// Skip a silent designated proposer.
    pub async fn skip(&self) -> Result<String, JsValue> {
        self.with(async |c| c.skip().await)
            .await
            .map(|h| h.to_base64url())
    }

    /// What the designated proposer carries forward from a dead round, if anything.
    #[wasm_bindgen(js_name = "lowestVotedInDeadRound")]
    pub fn lowest_voted_in_dead_round(&self) -> Result<Option<String>, JsValue> {
        self.now(|c| {
            Ok(c.lowest_voted_in_dead_round()
                .map_err(js)?
                .map(|h| h.to_base64url()))
        })
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
        self.with(async |c| c.propose_close(reason).await)
            .await
            .map(|h| h.to_base64url())
    }

    /// Propose closing on silent parties, by index.
    #[wasm_bindgen(js_name = "proposeAbandoned")]
    pub async fn propose_abandoned(&self, subjects: Vec<u32>) -> Result<String, JsValue> {
        let subjects: Vec<PartyIndex> = subjects.into_iter().map(|s| s as PartyIndex).collect();
        self.with(async |c| c.propose_abandoned(&subjects).await)
            .await
            .map(|h| h.to_base64url())
    }

    /// Confirm an abandoned close.
    #[wasm_bindgen(js_name = "confirmAbandoned")]
    pub async fn confirm_abandoned(&self, close: String) -> Result<String, JsValue> {
        let h = hash(&close)?;
        self.with(async |c| c.confirm_abandoned(h).await)
            .await
            .map(|h| h.to_base64url())
    }

    /// Reveal my commit-reveal nonce, when the rules want reveals.
    #[wasm_bindgen(js_name = "proposeReveal")]
    pub async fn propose_reveal(&self) -> Result<String, JsValue> {
        self.with(async |c| c.propose_reveal().await)
            .await
            .map(|h| h.to_base64url())
    }

    /// Copy committed links, QCs, receipts and engagements into my folder (§7). Returns how
    /// many files were written. `act` does this itself.
    pub async fn mirror(&self) -> Result<usize, JsValue> {
        self.with(async |c| c.mirror().await).await
    }

    /// Poll the known folders until a listing changes or `timeoutMs` passes.
    #[wasm_bindgen(js_name = "waitForChange")]
    pub async fn wait_for_change(&self, timeout_ms: u32) -> Result<bool, JsValue> {
        self.with(async |c| {
            c.wait_for_change(Duration::from_millis(u64::from(timeout_ms)))
                .await
        })
        .await
    }

    /// My index in genesis order; `undefined` before genesis is read or if I am not a party.
    #[wasm_bindgen(js_name = "myIndex")]
    pub fn my_index(&self) -> JsValue {
        let s = self.snapshot.borrow();
        let v = js_sys::Reflect::get(&s.session, &JsValue::from_str("my_index"))
            .unwrap_or(JsValue::UNDEFINED);
        if v.is_null() {
            JsValue::UNDEFINED
        } else {
            v
        }
    }

    /// The rules state at the head, or `undefined` before the rules initialise.
    pub fn state(&self) -> JsValue {
        self.snapshot.borrow().state.clone()
    }

    /// The terms genesis names — parties, rules, watchmen, quorum — once a sync has found the
    /// genesis link, including before it commits. `undefined` before that.
    pub fn arrangement(&self) -> JsValue {
        self.snapshot.borrow().arrangement.clone()
    }

    /// The chain as of the last call, or `undefined` before a sync.
    pub fn view(&self) -> JsValue {
        self.snapshot.borrow().view.clone()
    }

    /// Where I stand and what a page shows between calls: `{ chain, url, me, phase, parties,
    /// my_index, pending, held }` (§8.1, §6.4). `phase` is `loading`, `stranger`, `invited`,
    /// `waiting`, `open` or `ended`.
    pub fn session(&self) -> JsValue {
        self.snapshot.borrow().session.clone()
    }
}

/// Reads chains as anyone (§9, §14): lists the initiator's folder, reads the rules id from
/// genesis, verifies with the rules `resolve(id)` returns (a shipped id, a rules object, or
/// `undefined` for none), then walks every folder the head declares. Remembers decoded
/// records by content hash, so polling refetches only what changed.
#[wasm_bindgen]
pub struct ChainReader {
    store: JsStore,
    resolve: js_sys::Function,
    reader: Rc<Mutex<Reader>>,
}

#[wasm_bindgen]
impl ChainReader {
    /// A reader over `store` (read-only is fine) with a rules resolver.
    #[wasm_bindgen(constructor)]
    pub fn new(store: JsValue, resolve: js_sys::Function) -> Result<ChainReader, JsValue> {
        Ok(Self {
            store: JsStore::new(store)?,
            resolve,
            reader: Rc::new(Mutex::new(Reader::new())),
        })
    }

    /// Load a chain from a chain or record URL: `{ chain, owner, folder, url, rules,
    /// rules_known, view, view_error, folders, files }`.
    pub async fn load(&self, url: String) -> Result<JsValue, JsValue> {
        let mut r = self.reader.lock().await;
        let mut resolve_error: Option<JsValue> = None;
        let loaded = r
            .load(&self.store, &url, |id| {
                let v = self
                    .resolve
                    .call1(&JsValue::NULL, &JsValue::from_str(id))
                    .unwrap_or(JsValue::UNDEFINED);
                if v.is_undefined() || v.is_null() {
                    return None;
                }
                match rules(v) {
                    Ok(r) => Some(r),
                    Err(e) => {
                        resolve_error = Some(e);
                        None
                    }
                }
            })
            .await
            .map_err(js)?;
        if let Some(e) = resolve_error {
            return Err(e);
        }
        to_js(&loaded)
    }
}

/// Verify a chain from files alone, as anyone: no seat, no signer. `store.me` may be `""`.
/// `rules` is a shipped rules id or a rules object.
#[wasm_bindgen(js_name = "verifyFrom")]
pub async fn verify_from_url(
    rules: JsValue,
    store: JsValue,
    chain_url: String,
) -> Result<JsValue, JsValue> {
    let rules = self::rules(rules)?;
    let store = JsStore::new(store)?;
    let (chain, initiator) = parse_chain_url(&chain_url).map_err(js)?;
    let report = verify_from(&rules, &store, &chain, initiator)
        .await
        .map_err(js)?;
    to_js(&chain_view(&rules, &report.verdict, &report.suspects))
}

/// The chains `store.me` is on, from the index markers under `folder` (a protocol folder,
/// `/pub/<client_id>/mayfly/`; §7): `[{ url, finished }]`, finished first.
#[wasm_bindgen(js_name = "myChains")]
pub async fn my_chains_js(store: JsValue, folder: String) -> Result<JsValue, JsValue> {
    let store = JsStore::new(store)?;
    let chains = pubky_mayfly_client::my_chains(&store, &folder)
        .await
        .map_err(js)?;
    to_js(&chains)
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

/// Parse a chain URL, or a link to any record inside a chain folder, into `{ chain, owner,
/// folder, url }` with `url` the canonical chain URL.
#[wasm_bindgen(js_name = "parseChainUrl")]
pub fn parse_chain_url_js(url: String) -> Result<JsValue, JsValue> {
    let (chain, (owner, folder), url) =
        pubky_mayfly_client::reader::normalise_chain_url(&url).map_err(js)?;
    to_js(&serde_json::json!({
        "chain": chain.to_string(),
        "owner": owner,
        "folder": folder,
        "url": url,
    }))
}

/// The chain URL for `chain` under `owner`'s protocol folder.
#[wasm_bindgen(js_name = "chainUrl")]
pub fn chain_url_js(owner: String, folder: String, chain: String) -> Result<String, JsValue> {
    let id = ChainId::parse(&chain).map_err(|e| input(e.to_string()))?;
    let folder = pubky_mayfly_client::layout::Folder::from_path(&folder);
    Ok(format!("pubky://{owner}{}", folder.chain(&id).as_str()))
}

/// Whether `s` is a pubky: 52 characters of z-base-32 that decode to a public key.
#[wasm_bindgen(js_name = "isPubky")]
pub fn is_pubky(s: String) -> bool {
    pubky_mayfly::keys::parse_z32(&s).is_ok()
}

/// The rules ids this build ships. Other rules are passed as objects.
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
