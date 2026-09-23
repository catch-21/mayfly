//! The chain explorer (§14), demo edition: a JSON snapshot of every homeserver's files and the
//! verified chain, rebuilt after each step, served with a single-page viewer that polls it.
//!
//! Everything shown is read the way a bystander would read it — public listings and public
//! `GET`s through a seatless [`PubkyStore`] and [`verify_from`] — so the page shows what the
//! files prove, not what the demo believes.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::State;
use axum::response::{Html, Json};
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use tokio::sync::RwLock;

use pubky_mayfly::fold::{self, Status, Verdict};
use pubky_mayfly::hash::{ChainId, Hash};
use pubky_mayfly::record::{CloseBody, Kind};
use pubky_mayfly_client::chain::verify_from;
use pubky_mayfly_client::view::{decode_record, RecordView};
use pubky_mayfly_client::{PubkyStore, Store};
use pubky_mayfly_rules::list::List;

/// Someone with a homeserver folder in this story.
#[derive(Clone)]
pub struct Actor {
    pub name: String,
    pub role: &'static str,
    pub pubky: String,
    pub app: String,
    pub folder: String,
    pub kid: String,
}

#[derive(Serialize, Clone)]
pub struct FileView {
    pub path: String,
    pub what: String,
    pub h16: String,
    pub bytes: usize,
    pub step: usize,
    /// The file decoded as a record (§14, evidence panel); `None` for markers and keys.
    pub record: Option<RecordView>,
}

#[derive(Serialize, Clone)]
pub struct HomeserverView {
    pub name: String,
    pub role: &'static str,
    pub pubky: String,
    pub app: String,
    pub folder: String,
    pub online: bool,
    pub files: Vec<FileView>,
}

#[derive(Serialize, Clone)]
pub struct LinkView {
    pub seq: u64,
    pub round: u32,
    pub kind: String,
    pub summary: String,
    pub author: String,
    pub h16: String,
    pub confirmers: Vec<String>,
    pub witnessed: (usize, usize),
    pub is_final: bool,
}

#[derive(Serialize, Clone)]
pub struct CandidateView {
    pub h16: String,
    pub author: String,
    pub kind: String,
    pub summary: String,
    pub round: u32,
    pub votes: usize,
}

#[derive(Serialize, Clone)]
pub struct OpenSeqView {
    pub seq: u64,
    pub round: u32,
    pub dead: bool,
    pub candidates: Vec<CandidateView>,
    pub voters: Vec<(u32, Vec<String>)>,
    pub rejects: Vec<String>,
}

#[derive(Serialize, Clone)]
pub struct ItemView {
    pub id: String,
    pub text: String,
    pub ticked: bool,
}

#[derive(Serialize, Clone)]
pub struct AnomalyView {
    pub kind: String,
    pub against: String,
    pub seq: Option<u64>,
    pub evidence: Vec<String>,
}

#[derive(Serialize, Clone)]
pub struct ChainView {
    pub chain: String,
    pub status: String,
    pub is_final: bool,
    pub committed: Vec<LinkView>,
    pub open: Option<OpenSeqView>,
    pub items: Vec<ItemView>,
    pub archived: bool,
    pub anomalies: Vec<AnomalyView>,
    pub suspects: Vec<(String, String)>,
    pub engaged: Vec<(String, u64)>,
}

#[derive(Serialize, Clone)]
pub struct StepLog {
    pub n: usize,
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Serialize, Clone, Default)]
pub struct Snapshot {
    pub step: usize,
    pub title: String,
    pub finished: bool,
    pub homeservers: Vec<HomeserverView>,
    pub chain: Option<ChainView>,
    pub log: Vec<StepLog>,
}

/// Builds snapshots: remembers when each file first appeared so the page can flash it.
pub struct Explorer {
    observer: PubkyStore,
    actors: Vec<Actor>,
    first_seen: BTreeMap<(String, String), usize>,
    shared: Arc<RwLock<Snapshot>>,
}

impl Explorer {
    pub fn new(observer: PubkyStore, actors: Vec<Actor>) -> Self {
        Self {
            observer,
            actors,
            first_seen: BTreeMap::new(),
            shared: Arc::new(RwLock::new(Snapshot::default())),
        }
    }

    /// Party names in genesis order are the first `n` actors with role `party`.
    fn party_names(&self) -> Vec<String> {
        self.actors
            .iter()
            .filter(|a| a.role == "party")
            .map(|a| a.name.clone())
            .collect()
    }

    fn name_of_pubky(&self, pubky: &str) -> String {
        self.actors
            .iter()
            .find(|a| a.pubky == pubky)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| crate::ui::short(pubky))
    }

    fn name_of_kid(&self, kid: &str, verdict: &Verdict) -> String {
        if let Some(seat) = verdict.seats.iter().find(|s| s.kid == kid) {
            return self.name_of_pubky(&seat.pubky);
        }
        if let Some(w) = verdict.engaged.iter().find(|w| w.kid == kid) {
            return self.name_of_pubky(&w.pubky);
        }
        self.actors
            .iter()
            .find(|a| a.kid == kid)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| crate::ui::short(kid))
    }

    /// Read every homeserver and verify the chain; publish the snapshot; return the files that
    /// are new since the last call, per homeserver, and the verdict.
    pub async fn refresh(
        &mut self,
        step: usize,
        title: &str,
        chain: Option<(&ChainId, (String, String))>,
        offline: &[String],
        log: &[StepLog],
        finished: bool,
    ) -> (Vec<(String, Vec<FileView>)>, Option<Verdict>) {
        let mut homeservers = Vec::new();
        let mut fresh = Vec::new();
        for a in &self.actors {
            let mut files = Vec::new();
            let mut new_here = Vec::new();
            let listed = self
                .observer
                .list(&a.pubky, &a.folder)
                .await
                .unwrap_or_default();
            for l in listed {
                let bytes = self
                    .observer
                    .get(&a.pubky, &l.path)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let key = (a.pubky.clone(), l.path.clone());
                let first = *self.first_seen.entry(key).or_insert(step);
                let rel = l.path[a.folder.len()..].to_string();
                let what = describe(&rel);
                let record = if rel.ends_with(".jws") && !rel.starts_with("keys/") {
                    Some(decode_record(&bytes, &rel, l.content_hash.as_ref()))
                } else {
                    None
                };
                let view = FileView {
                    what,
                    path: rel,
                    h16: Hash::of(&bytes).h16(),
                    bytes: bytes.len(),
                    step: first,
                    record,
                };
                if first == step {
                    new_here.push(view.clone());
                }
                files.push(view);
            }
            fresh.push((a.name.clone(), new_here));
            homeservers.push(HomeserverView {
                name: a.name.clone(),
                role: a.role,
                pubky: a.pubky.clone(),
                app: a.app.clone(),
                folder: a.folder.clone(),
                online: !offline.contains(&a.name),
                files,
            });
        }

        let mut verdict_out = None;
        let chain_view = match chain {
            Some((id, initiator)) => {
                match verify_from(&List, &self.observer, id, initiator).await {
                    Ok(report) => {
                        let view =
                            self.chain_view(id, &report.verdict, &report.suspects, &homeservers);
                        verdict_out = Some(report.verdict);
                        Some(view)
                    }
                    Err(_) => None,
                }
            }
            None => None,
        };

        let snapshot = Snapshot {
            step,
            title: title.to_string(),
            finished,
            homeservers,
            chain: chain_view,
            log: log.to_vec(),
        };
        *self.shared.write().await = snapshot;
        (fresh, verdict_out)
    }

    fn chain_view(
        &self,
        id: &ChainId,
        v: &Verdict,
        suspects: &[pubky_mayfly_client::Listed],
        homeservers: &[HomeserverView],
    ) -> ChainView {
        let parties = self.party_names();
        let name = |i: usize| {
            parties
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("party {i}"))
        };
        let committed = v
            .committed
            .iter()
            .map(|c| LinkView {
                seq: c.link.payload.seq,
                round: c.link.payload.round,
                kind: c.link.payload.kind.clone(),
                summary: summary(&c.link.payload.kind, &c.link.payload.body, &parties, v),
                author: name(c.author),
                h16: c.link.hash.h16(),
                confirmers: c
                    .qc
                    .iter()
                    .map(|q| self.name_of_kid(&q.payload.kid, v))
                    .collect(),
                witnessed: c.witnessed,
                is_final: c.is_final,
            })
            .collect();
        let open = v.open.as_ref().map(|o| OpenSeqView {
            seq: o.seq,
            round: o.round,
            dead: o.dead,
            candidates: o
                .candidates
                .iter()
                .map(|c| CandidateView {
                    h16: c.hash.h16(),
                    author: name(c.author),
                    kind: c.kind.clone(),
                    summary: c.kind.clone(),
                    round: c.round,
                    votes: c.votes,
                })
                .collect(),
            voters: o
                .voters
                .iter()
                .map(|(r, ps)| (*r, ps.iter().map(|p| name(*p)).collect()))
                .collect(),
            rejects: homeservers
                .iter()
                .flat_map(|h| {
                    let at_seq = format!("/rejects/{:08}-", o.seq);
                    h.files
                        .iter()
                        .filter(move |f| f.path.contains(&at_seq))
                        .map(move |f| {
                            format!(
                                "{} {}",
                                h.name,
                                f.path.rsplit('/').next().unwrap_or_default()
                            )
                        })
                })
                .collect(),
        });
        let (items, archived) = match fold::replay_state(&List, v) {
            Ok(Some(s)) => (
                s.items
                    .iter()
                    .map(|i| ItemView {
                        id: i.id.clone(),
                        text: i.text.clone(),
                        ticked: i.ticked,
                    })
                    .collect(),
                s.archived,
            ),
            _ => (Vec::new(), false),
        };
        ChainView {
            chain: id.to_string(),
            status: status_line(&v.status, &parties),
            is_final: v.is_final(),
            committed,
            open,
            items,
            archived,
            anomalies: v
                .anomalies
                .iter()
                .map(|a| AnomalyView {
                    kind: format!("{:?}", a.kind),
                    against: a
                        .against
                        .as_deref()
                        .map(|k| self.name_of_kid(k, v))
                        .unwrap_or_else(|| "a source".into()),
                    seq: a.seq,
                    evidence: a.evidence.iter().map(Hash::h16).collect(),
                })
                .collect(),
            suspects: suspects
                .iter()
                .map(|s| {
                    (
                        self.name_of_pubky(&s.owner),
                        s.path.rsplit('/').next().unwrap_or_default().to_string(),
                    )
                })
                .collect(),
            engaged: v
                .engaged
                .iter()
                .map(|w| (self.name_of_pubky(&w.pubky), w.until))
                .collect(),
        }
    }

    /// Serve the viewer and the snapshot on `port`.
    pub fn serve(&self, port: u16) -> tokio::task::JoinHandle<()> {
        let shared = Arc::clone(&self.shared);
        tokio::spawn(async move {
            let app = Router::new()
                .route("/", get(|| async { Html(include_str!("index.html")) }))
                .route(
                    "/state.json",
                    get(|State(s): State<Arc<RwLock<Snapshot>>>| async move {
                        Json(s.read().await.clone())
                    }),
                )
                .with_state(shared);
            match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                Ok(listener) => {
                    let _ = axum::serve(listener, app).await;
                }
                Err(e) => eprintln!("explorer: cannot bind port {port}: {e}"),
            }
        })
    }
}

/// What a file is, from where it sits (§7).
fn describe(rel: &str) -> String {
    let segs: Vec<&str> = rel.split('/').collect();
    match segs.as_slice() {
        ["chains", _, "links", n] if n.starts_with("00000000-") => "genesis link".into(),
        ["chains", _, "links", _] => "link".into(),
        ["chains", _, "confirms", n] if n.matches('-').count() == 2 => {
            "mirrored confirmation".into()
        }
        ["chains", _, "confirms", _] => "confirmation".into(),
        ["chains", _, "rejects", _] => "reject".into(),
        ["chains", _, "receipts", _, "engage.jws"] => "mirrored engagement".into(),
        ["chains", _, "receipts", _, _] => "mirrored receipt".into(),
        ["witness", _, "engage.jws"] => "engagement".into(),
        ["witness", _, "engage", _] => "engagement (historic)".into(),
        ["witness", _, _] => "receipt".into(),
        ["index", state, _] => format!("{state} marker"),
        ["keys", _] => "key".into(),
        _ => "file".into(),
    }
}

/// One line for a link's content.
pub fn summary(kind: &str, body: &serde_json::Value, parties: &[String], v: &Verdict) -> String {
    let text = |k: &str| {
        body.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    match Kind::parse(kind) {
        Kind::Rules => match kind {
            "genesis" => {
                let g = v.genesis();
                format!(
                    "{} parties, quorum {}, {} witness{}",
                    parties.len(),
                    g.as_ref().map(|g| g.confirm_quorum).unwrap_or_default(),
                    g.as_ref().map(|g| g.witnesses.len()).unwrap_or_default(),
                    if g.as_ref().map(|g| g.witnesses.len()).unwrap_or_default() == 1 {
                        ""
                    } else {
                        "es"
                    }
                )
            }
            "add" => format!("add “{}”", text("text")),
            "tick" | "untick" | "remove" => format!("{kind} {}", text("id")),
            "edit" => format!("edit {} → “{}”", text("id"), text("text")),
            _ => kind.to_string(),
        },
        Kind::Close => match serde_json::from_value::<CloseBody>(body.clone()) {
            Ok(c) => {
                let subjects: Vec<String> = c
                    .subject
                    .iter()
                    .map(|s| {
                        v.genesis()
                            .and_then(|g| g.parties.iter().position(|p| p.pubky == *s))
                            .and_then(|i| parties.get(i).cloned())
                            .unwrap_or_else(|| crate::ui::short(s))
                    })
                    .collect();
                if subjects.is_empty() {
                    format!("close ({:?})", c.reason).to_lowercase()
                } else {
                    format!("close (abandoned by {})", subjects.join(", "))
                }
            }
            Err(_) => "close".into(),
        },
        k => format!("{k:?}").to_lowercase(),
    }
}

/// One line for the status.
pub fn status_line(s: &Status, parties: &[String]) -> String {
    let name = |i: &usize| {
        parties
            .get(*i)
            .cloned()
            .unwrap_or_else(|| format!("party {i}"))
    };
    match s {
        Status::Ongoing => "ongoing".into(),
        Status::Stalled {
            seq,
            round,
            dead,
            awaiting,
        } => format!(
            "stalled at seq {seq}, round {round}{} — awaiting {}",
            if *dead { " (dead)" } else { "" },
            awaiting.iter().map(name).collect::<Vec<_>>().join(", ")
        ),
        Status::Paused { seq, close } => {
            format!("paused at seq {seq}: abandoned close {close:?}").to_lowercase()
        }
        Status::Closed(o) => format!("closed — {}", o.summary),
        Status::Abandoned { subjects, outcome } => format!(
            "abandoned by {} — {}",
            subjects.iter().map(name).collect::<Vec<_>>().join(", "),
            outcome.summary
        ),
    }
}
