//! A shopping list on Mayfly, told step by step on a local Pubky testnet.
//!
//! Three people on three homeservers keep one list. Every append is a signed record in the
//! author's own storage, committed when the others confirm it, receipted by a watchdog for
//! time, and verifiable by anyone from the files. The demo walks the happy paths (create, join,
//! append, converge) and the sad ones (a rule refusing a proposal, a forged record, competing
//! proposals, a tampered mirror, a party going quiet and being closed out) — pausing for a key
//! press between steps while a browser page shows every homeserver and the verified chain.
//!
//! ```text
//! TEST_PUBKY_CONNECTION_STRING='postgres://postgres:postgres@localhost:5432/postgres' \
//!   cargo run -p mayfly-demo                        # Docker Postgres on 5432
//! cargo run -p mayfly-demo -- --auto --port 8787    # unattended; --pause-secs N to linger
//! ```
//!
//! The testnet is a [`StaticTestnet`]: fixed, well-known ports (pkarr relay `15411`, homeserver
//! `6286`/`6287`), so that the Pubky explorer's testnet mode — <https://explorer.pubky.app/testnet/>
//! — can browse the same homeserver while the demo runs.

mod explorer;
mod ui;

use std::time::Duration;

use pubky_testnet::pubky::{ClientId, Keypair, Pubky, PublicKey};
use pubky_testnet::StaticTestnet;
use serde_json::json;

use pubky_mayfly::fold::{Status, Verdict};
use pubky_mayfly::hash::{ChainId, Hash};
use pubky_mayfly::record::Link;
use pubky_mayfly::{typ, PROTOCOL_VERSION};
use pubky_mayfly_client::chain::verify_from;
use pubky_mayfly_client::layout::Folder;
use pubky_mayfly_client::{
    Action, ChainClient, Error, GenesisSpec, PubkyStore, SessionSigner, Signer, Store,
};
use pubky_mayfly_rules::list::{Body, List};
use pubky_mayfly_watchdog::{Operator, Terms};

use explorer::{Actor, Explorer, StepLog};

type Client = ChainClient<List, PubkyStore, SessionSigner>;

/// The rules' response allowance for this demo: a party silent this long can be closed out.
const RESPOND_MS: u64 = 5_000;
/// A silent designated proposer is skipped after this long; generous, so key presses never
/// trigger it.
const THINK_MS: u64 = 120_000;

struct Person {
    name: &'static str,
    store: PubkyStore,
    signer: SessionSigner,
}

struct Demo {
    auto: bool,
    pause_secs: u64,
    explorer: Explorer,
    operator: Operator<PubkyStore, SessionSigner>,
    people: Vec<Person>,
    clients: Vec<Client>,
    chain: Option<ChainId>,
    initiator: Option<(String, String)>,
    offline: Vec<String>,
    log: Vec<StepLog>,
    step: usize,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn session(pubky: &Pubky, homeserver: &PublicKey, app: &str) -> (PubkyStore, SessionSigner) {
    let signer = pubky.signer(Keypair::random());
    signer.signup(homeserver, None).await.expect("signup");
    let session = signer
        .signin(ClientId::new(app).expect("client id"))
        .await
        .expect("signin");
    let store = PubkyStore::new(pubky.clone(), session.clone());
    let signer = SessionSigner::from_session(&session)
        .await
        .expect("grant session");
    (store, signer)
}

impl Demo {
    fn begin(&mut self, title: &str) {
        self.step += 1;
        ui::heading(self.step, title);
        self.log.push(StepLog {
            n: self.step,
            title: title.to_string(),
            lines: Vec::new(),
        });
    }

    fn record(&mut self, line: String) {
        if let Some(l) = self.log.last_mut() {
            l.lines.push(line);
        }
    }

    fn say(&mut self, text: &str) {
        ui::say(text);
        self.record(text.to_string());
    }

    fn ok(&mut self, text: &str) {
        ui::ok(text);
        self.record(format!("✔ {text}"));
    }

    fn refused(&mut self, text: &str) {
        ui::refused(text);
        self.record(format!("✘ {text}"));
    }

    fn note(&mut self, text: &str) {
        ui::note(text);
        self.record(format!("! {text}"));
    }

    fn watchdog(&mut self, text: &str) {
        ui::watchdog(text);
        self.record(format!("👁 {text}"));
    }

    fn names(&self) -> Vec<String> {
        self.people.iter().map(|p| p.name.to_string()).collect()
    }

    async fn sweep(&mut self) -> pubky_mayfly_watchdog::SweepReport {
        self.operator.sweep().await.expect("watchdog sweep")
    }

    /// Everyone online acts; returns `(name, actions)` for those who did anything.
    async fn everyone_acts(&mut self) -> Vec<(String, Vec<Action>)> {
        let mut out = Vec::new();
        for i in 0..self.clients.len() {
            let name = self.people[i].name.to_string();
            if self.offline.contains(&name) {
                continue;
            }
            let acts = self.clients[i].act().await.expect("act");
            if !acts.is_empty() {
                out.push((name, acts));
            }
        }
        out
    }

    fn describe_actions(&mut self, acted: &[(String, Vec<Action>)]) {
        for (who, acts) in acted {
            for a in acts {
                let line = match a {
                    Action::Confirmed(h) => format!("{who} confirmed {}", h.h16()),
                    Action::Reproposed { earlier, link } => {
                        format!("{who} re-proposed {} as {}", earlier.h16(), link.h16())
                    }
                    Action::Passed(_) => format!("{who} passed"),
                    Action::Skipped(_) => format!("{who} skipped a silent proposer"),
                    Action::Rejected { link, reason } => {
                        format!("{who} rejected {} — {reason}", link.h16())
                    }
                    Action::Decision { candidate, .. } => {
                        format!(
                            "{who} was asked to decide on a {} by {}",
                            candidate.kind, self.people[candidate.author].name
                        )
                    }
                    Action::MyTurn { round } => {
                        format!("{who}: round {round} is theirs to propose in")
                    }
                    Action::AwaitingWitnesses { have, of, .. } => {
                        format!("{who} is waiting for witnesses ({have}/{of})")
                    }
                };
                self.ok(&line);
            }
        }
    }

    /// Act repeatedly until the chain has `len` committed links (bounded).
    async fn settle(&mut self, len: usize) {
        for _ in 0..8 {
            let acted = self.everyone_acts().await;
            self.describe_actions(&acted);
            self.sweep().await;
            let done = self.clients[0]
                .verdict()
                .map(|v| v.committed.len() >= len)
                .unwrap_or(false);
            if done {
                let acted = self.everyone_acts().await;
                self.describe_actions(&acted);
                return;
            }
        }
    }

    /// Refresh the explorer, print what landed where and the chain, and wait for a key.
    async fn end(&mut self, finished: bool) -> Option<Verdict> {
        self.sweep().await;
        let title = self.log.last().map(|l| l.title.clone()).unwrap_or_default();
        let chain = self.chain.as_ref().zip(self.initiator.clone());
        let (fresh, verdict) = self
            .explorer
            .refresh(self.step, &title, chain, &self.offline, &self.log, finished)
            .await;
        let any = fresh.iter().any(|(_, f)| !f.is_empty());
        if any {
            ui::gap();
            println!("  {}new on the homeservers:{}", ui::DIM, ui::RESET);
            for (name, files) in &fresh {
                for f in files {
                    println!(
                        "  {}{name:>6}{}  {:<24} {}{}{}",
                        ui::BOLD,
                        ui::RESET,
                        f.what,
                        ui::DIM,
                        f.path,
                        ui::RESET
                    );
                }
            }
        }
        if let Some(v) = &verdict {
            ui::gap();
            self.print_chain(v);
        }
        ui::gap();
        ui::pause(self.auto, self.pause_secs).await;
        verdict
    }

    fn print_chain(&self, v: &Verdict) {
        let names = self.names();
        let name = |i: usize| names.get(i).cloned().unwrap_or_default();
        for c in &v.committed {
            let confirmers: Vec<String> =
                c.qc.iter()
                    .map(|q| {
                        v.seats
                            .iter()
                            .find(|s| s.kid == q.payload.kid)
                            .and_then(|s| self.people.iter().find(|p| p.signer.pubky() == s.pubky))
                            .map(|p| p.name.to_string())
                            .unwrap_or_else(|| ui::short(&q.payload.kid))
                    })
                    .collect();
            let witnessed = if c.witnessed.1 > 0 {
                format!(
                    "  {}👁 {}/{}{}",
                    if c.witnessed.0 > 0 {
                        ui::MAGENTA
                    } else {
                        ui::YELLOW
                    },
                    c.witnessed.0,
                    c.witnessed.1,
                    ui::RESET
                )
            } else {
                String::new()
            };
            println!(
                "  {}#{:<2}{} {:<30} {}by {:<6}{} {}✓ {}{}{}{}",
                ui::BOLD,
                c.link.payload.seq,
                ui::RESET,
                explorer::summary(&c.link.payload.kind, &c.link.payload.body, &names, v),
                ui::DIM,
                name(c.author),
                ui::RESET,
                ui::GREEN,
                confirmers.join(", "),
                ui::RESET,
                witnessed,
                if c.is_final {
                    String::new()
                } else {
                    format!("  {}head{}", ui::DIM, ui::RESET)
                }
            );
        }
        if let Some(o) = &v.open {
            let mut line = format!(
                "  {}#{:<2} open, round {}{}{}",
                ui::YELLOW,
                o.seq,
                o.round,
                if o.dead { " (dead)" } else { "" },
                ui::RESET
            );
            for c in &o.candidates {
                line.push_str(&format!(
                    "  {}{} by {} r{} ({} vote{}){}",
                    ui::DIM,
                    c.kind,
                    name(c.author),
                    c.round,
                    c.votes,
                    if c.votes == 1 { "" } else { "s" },
                    ui::RESET
                ));
            }
            println!("{line}");
        }
        println!(
            "  {}status: {}{}",
            ui::DIM,
            explorer::status_line(&v.status, &names),
            ui::RESET
        );
    }

    fn my_folder(&self, i: usize) -> Folder {
        Folder::from_path(&self.people[i].signer.path())
    }
}

#[tokio::main]
async fn main() {
    // `RUST_LOG=pubky=warn` shows the SDK's transport decisions, e.g. an ICANN fallback.
    if std::env::var_os("RUST_LOG").is_some() {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .init();
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let auto = args.iter().any(|a| a == "--auto");
    let port: u16 = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse().ok())
        .unwrap_or(8787);
    let pause_secs: u64 = args
        .iter()
        .position(|a| a == "--pause-secs")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);

    println!(
        "{}{}Mayfly — a shopping list among three people, on Pubky{}",
        ui::BOLD,
        ui::CYAN,
        ui::RESET
    );
    ui::say("Starting a local Pubky testnet on its well-known ports: pkarr relay 15411, homeserver 6286 (HTTP) and 6287 (pubky TLS). The homeserver needs Postgres. With the Docker one-liner, set TEST_PUBKY_CONNECTION_STRING so the URL includes user postgres and password postgres — the default URL uses your OS user and will fail.");
    let testnet = match StaticTestnet::builder().build().await {
        Ok(t) => t,
        Err(e) => {
            ui::refused(&format!("the testnet did not start: {e}"));
            ui::say("If a port is taken, stop whatever holds it (another testnet, a Docker homeserver-testnet). For Docker Postgres: `docker run --name pubky-postgres -e POSTGRES_USER=postgres -e POSTGRES_PASSWORD=postgres -p 127.0.0.1:5432:5432 -d postgres:18` then TEST_PUBKY_CONNECTION_STRING='postgres://postgres:postgres@localhost:5432/postgres' cargo run -p mayfly-demo. If you published a different host port, put that port in the URL.");
            std::process::exit(1);
        }
    };
    let homeserver = testnet.homeserver_app().public_key();
    let pubky = testnet.sdk().expect("sdk");
    ui::ok("testnet up");

    // ── Step 1 ────────────────────────────────────────────────────────────────────────────────
    let apps = [
        ("Alice", "groceries.example"),
        ("Bob", "groceries.example"),
        ("Carol", "pantry.example"),
    ];
    let mut people = Vec::new();
    for (name, app) in apps {
        let (store, signer) = session(&pubky, &homeserver, app).await;
        people.push(Person {
            name,
            store,
            signer,
        });
    }
    let (dog_store, dog_signer) = session(&pubky, &homeserver, "watchdog.example").await;
    let mut actors: Vec<Actor> = people
        .iter()
        .map(|p| Actor {
            name: p.name.into(),
            role: "party",
            pubky: p.signer.pubky(),
            app: p.signer.client_id(),
            folder: p.signer.path(),
            kid: p.signer.kid(),
        })
        .collect();
    actors.push(Actor {
        name: "Wendy".into(),
        role: "watchdog",
        pubky: dog_signer.pubky(),
        app: dog_signer.client_id(),
        folder: dog_signer.path(),
        kid: dog_signer.kid(),
    });
    let explorer = Explorer::new(PubkyStore::read_only(testnet.sdk().expect("sdk")), actors);
    let _server = explorer.serve(port);
    let mut operator = Operator::new(
        dog_store,
        dog_signer.clone(),
        Terms::receipts(0).poll_every(2_000),
    )
    .engage_for(3_600)
    .renew_before(600);
    operator.free(people[0].signer.pubky());

    let mut demo = Demo {
        auto,
        pause_secs,
        explorer,
        operator,
        people,
        clients: Vec::new(),
        chain: None,
        initiator: None,
        offline: Vec::new(),
        log: Vec::new(),
        step: 0,
    };

    demo.begin("Three people, three apps, one homeserver each");
    demo.say(&format!("Open the explorer at http://127.0.0.1:{port}/ — it reads the homeservers the way any bystander could and shows the files and the verified chain. The Pubky explorer at https://explorer.pubky.app/testnet/ can browse the same homeserver's raw files through the relay on 15411."));
    ui::gap();
    for i in 0..3 {
        let p = &demo.people[i];
        ui::kv(
            p.name,
            &format!(
                "pubky {}  app {}  folder {}",
                ui::short(&p.signer.pubky()),
                p.signer.client_id(),
                p.signer.path()
            ),
        );
    }
    ui::kv(
        "Wendy",
        &format!(
            "pubky {}  app {}  (the watchdog)",
            ui::short(&dog_signer.pubky()),
            dog_signer.client_id()
        ),
    );
    ui::gap();
    demo.say("Each person signed in to an app with a Pubky Grant. The Grant's client key is the chain key: it signs every record, and anyone can check the Grant binds it to the person's pubky (§5). Nobody has a shared server; each writes only to their own /pub/ folder.");
    demo.say("Wendy is a watchdog: an impartial third party who receipts what she sees with her clock. She watches Alice's chains for free — this is a demo.");
    demo.end(false).await;

    // ── Step 2 ────────────────────────────────────────────────────────────────────────────────
    demo.begin("Alice starts the list");
    let mut spec = GenesisSpec::new(demo.people.iter().map(|p| p.signer.pubky()).collect())
        .with_apps(&["groceries.example", "groceries.example", "pantry.example"]);
    spec.witnesses = vec![dog_signer.pubky()];
    spec.options = json!({ "time_control": { "think_ms": THINK_MS, "respond_ms": RESPOND_MS } });
    let alice = ChainClient::create(
        List,
        demo.people[0].store.clone(),
        demo.people[0].signer.clone(),
        spec,
    )
    .await
    .expect("genesis");
    let chain = alice.chain().clone();
    let invite = alice.invite_url();
    demo.chain = Some(chain.clone());
    demo.initiator = Some((demo.people[0].signer.pubky(), demo.people[0].signer.path()));
    demo.clients.push(alice);
    demo.say("Alice's app writes a genesis link naming the three parties, the rules (list/1), unanimity as the quorum, Wendy as witness, and a 5-second response allowance so the demo can show a party being closed out. Its id is derived from its bytes.");
    ui::kv("chain id", chain.as_str());
    ui::kv("invite", &invite);
    demo.say("Genesis is Alice's proposal and her vote. Nothing is committed until the others confirm. Her client also wrote an index/active marker — a UI listing entry, and what Wendy acts on.");
    demo.end(false).await;

    // ── Step 3 ────────────────────────────────────────────────────────────────────────────────
    demo.begin("Wendy notices");
    demo.say("Nobody asks Wendy to watch. She reads her customers' /pub/ folders for index/active markers, follows the one Alice wrote to the genesis, sees she is named, and publishes an engagement (§11.2) — before Bob and Carol have even been invited.");
    let mut engaged = false;
    for _ in 0..10 {
        let report = demo.sweep().await;
        if report.engaged.contains(&chain) || demo.operator.watching().any(|(c, _)| *c == chain) {
            engaged = true;
            break;
        }
        if !report.declined.is_empty() {
            demo.note(&format!("declined: {:?}", report.declined));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    if engaged {
        demo.watchdog("engaged: witness/<chain>/engage.jws under Wendy's own folder, with her Grant embedded so anyone can verify her key");
    } else {
        demo.note("Wendy has not found the marker yet; she polls, and will");
    }
    demo.end(false).await;

    // ── Step 4 ────────────────────────────────────────────────────────────────────────────────
    demo.begin("Bob and Carol join");
    demo.say("Each opens the invite URL, reads the genesis from Alice's folder, and reviews it: who is in, what rules, what quorum, which witness. Joining is consent, so a real app shows this and asks. Then each confirms genesis with their own Grant and folder.");
    for i in 1..3 {
        let mut c = ChainClient::open_url(
            List,
            demo.people[i].store.clone(),
            demo.people[i].signer.clone(),
            &invite,
        )
        .expect("invite");
        c.sync().await.expect("sync");
        c.join().await.expect("join");
        demo.clients.push(c);
        let name = demo.people[i].name;
        demo.ok(&format!(
            "{name} confirmed genesis from {}",
            demo.people[i].signer.path()
        ));
    }
    demo.say("With all three votes in one round, genesis is committed. Everyone runs act(): it syncs, mirrors the committed link and its confirmations into their own folder, and finds Wendy's engagement — from her /pub/ alone — so her receipts count.");
    demo.settle(1).await;
    demo.end(false).await;

    // ── Step 5 ────────────────────────────────────────────────────────────────────────────────
    demo.begin("Alice adds Milk");
    demo.say("Alice proposes a typed body. Her client runs the rules locally first, then writes links/00000001-<hash>.jws embedding the confirmations that committed genesis — the quorum certificate — so the chain proves its own history.");
    demo.clients[0]
        .propose_body(&Body::Add {
            id: "milk".into(),
            text: "Milk".into(),
            qty: None,
        })
        .await
        .expect("propose");
    demo.say("Bob's and Carol's act() see a valid proposal in their round and confirm it. Wendy receipts the proposal and each confirmation, in causal order, with her clock.");
    demo.settle(2).await;
    let v = demo.end(false).await.expect("verdict");
    let _ = v;

    // ── Step 6 ────────────────────────────────────────────────────────────────────────────────
    demo.begin("The rules say no");
    demo.say("Bob tries to add an item with an id that is already taken, then to tick one that does not exist. The rules run before anything is written: both are refused on his machine and nothing reaches his homeserver.");
    match demo.clients[1]
        .propose_body(&Body::Add {
            id: "milk".into(),
            text: "More milk".into(),
            qty: None,
        })
        .await
    {
        Err(Error::Rules(m)) => demo.refused(&format!("Bob's duplicate add refused — {m}")),
        other => demo.note(&format!("unexpected: {other:?}")),
    }
    match demo.clients[1]
        .propose_body(&Body::Tick {
            id: "unicorn".into(),
        })
        .await
    {
        Err(Error::Rules(m)) => {
            demo.refused(&format!("Bob's tick of a missing item refused — {m}"))
        }
        other => demo.note(&format!("unexpected: {other:?}")),
    }
    demo.say("Had Bob's client been dishonest and written the link anyway, the others' folds would not admit it as a candidate — which is the next step.");
    demo.end(false).await;

    // ── Step 7 ────────────────────────────────────────────────────────────────────────────────
    demo.begin("A forged record");
    demo.say("Carol bypasses her client and hand-signs a link ticking an item that does not exist, correctly chained onto the head, and puts it in her own links/ folder. It is validly signed by her key — the rules are what refuse it.");
    let forged = {
        let v = demo.clients[2].verdict().expect("verdict").clone();
        let head = v.head().expect("head");
        let open = v.open.as_ref().expect("open");
        let mut confirms: Vec<(String, String)> = head
            .qc
            .iter()
            .map(|c| {
                (
                    c.payload.kid.clone(),
                    String::from_utf8(c.bytes.clone()).expect("ascii"),
                )
            })
            .collect();
        confirms.sort();
        let link = Link {
            v: PROTOCOL_VERSION,
            chain: chain.clone(),
            seq: open.seq,
            round: 0,
            prev: head.link.hash.to_base64url(),
            confirms: confirms.into_iter().map(|(_, c)| c).collect(),
            receipts: Vec::new(),
            author: demo.people[2].signer.pubky(),
            kid: demo.people[2].signer.kid(),
            ts: now_ms(),
            kind: "tick".into(),
            body: json!({ "id": "unicorn" }),
            state: head.link.payload.state.clone(),
            grant: None,
        };
        let jws = demo.people[2]
            .signer
            .sign_jws(typ::LINK, serde_json::to_value(&link).expect("json"))
            .await
            .expect("sign");
        let bytes = jws.into_bytes();
        let hash = Hash::of(&bytes);
        let path = demo.my_folder(2).chain(&chain).link(open.seq, &hash);
        demo.people[2].store.put(&path, bytes).await.expect("put");
        hash
    };
    demo.refused(&format!(
        "Carol wrote {} by hand: tick “unicorn”",
        forged.h16()
    ));
    demo.say("Alice's act() finds a link in her round that the fold does not admit as a candidate. Per §8.2 she rejects it, naming it, and keeps the bytes as evidence. One reject makes unanimity impossible: round 0 is dead and rotation names a proposer for round 1.");
    let acted = demo.everyone_acts().await;
    demo.describe_actions(&acted);
    let designated = {
        let v = demo.clients[0].verdict().expect("verdict");
        let open = v.open.as_ref().expect("open");
        pubky_mayfly::vote::designated(&chain, open.seq, 1, 3)
    };
    let d_name = demo.people[designated].name;
    demo.say(&format!("Round 1 is {d_name}'s to propose in. Nothing valid received a vote in round 0, so there is nothing to carry forward; {d_name} proposes Bread instead, in round 1."));
    demo.clients[designated]
        .propose_body(&Body::Add {
            id: "bread".into(),
            text: "Bread".into(),
            qty: None,
        })
        .await
        .expect("propose in round 1");
    demo.settle(3).await;
    demo.say("The forged link stays in Carol's folder as what it is: a signed record that no verifier admits, beside Alice's signed reject of it.");
    demo.end(false).await;

    // ── Step 8 ────────────────────────────────────────────────────────────────────────────────
    demo.begin("Two proposals at once");
    demo.say("Alice and Bob each propose before seeing the other. Both are valid; each carries its author's vote for a different link, so round 0 can never be unanimous: it is dead the moment anyone sees both. Nobody's vote is taken back.");
    let la = demo.clients[0]
        .propose_body(&Body::Add {
            id: "eggs".into(),
            text: "Eggs".into(),
            qty: None,
        })
        .await
        .expect("propose");
    let lb = demo.clients[1]
        .propose_body(&Body::Add {
            id: "butter".into(),
            text: "Butter".into(),
            qty: None,
        })
        .await
        .expect("propose");
    let (winner, loser_item) = if la < lb {
        ("Eggs", ("butter", "Butter"))
    } else {
        ("Butter", ("eggs", "Eggs"))
    };
    demo.say(&format!("Round 1 has one designated proposer, who re-proposes the lowest-hash content from round 0 — {winner} — and the others confirm. The other author simply proposes again at the next seq. Users see a brief merge, not a conflict dialogue."));
    let before = demo.clients[0]
        .verdict()
        .map(|v| v.committed.len())
        .unwrap_or(0);
    demo.settle(before + 1).await;
    let loser = if la < lb { 1 } else { 0 };
    demo.clients[loser]
        .propose_body(&Body::Add {
            id: loser_item.0.into(),
            text: loser_item.1.into(),
            qty: None,
        })
        .await
        .expect("re-propose");
    demo.settle(before + 2).await;
    demo.end(false).await;

    // ── Step 9 ────────────────────────────────────────────────────────────────────────────────
    demo.begin("A tampered mirror");
    demo.say("Every party mirrors committed links into their own folder. Carol edits her copy of link #1 in place: same file name, different bytes. A filename is never identity — everyone hashes what they fetch — so the copy is flagged as hers and ignored, and the chain is unchanged.");
    {
        let l1 = demo.clients[0].verdict().expect("verdict").committed[1]
            .link
            .hash;
        let path = demo.my_folder(2).chain(&chain).link(1, &l1);
        let mut bytes = demo.people[2]
            .store
            .get(&demo.people[2].signer.pubky(), &path)
            .await
            .expect("get")
            .expect("mirror");
        let at = bytes.len() * 3 / 5;
        bytes[at] ^= 0x01;
        demo.people[2].store.put(&path, bytes).await.expect("put");
    }
    demo.refused("Carol overwrote her mirror of link #1 with altered bytes");
    let report = demo.clients[0].sync().await.expect("sync");
    for s in &report.suspects {
        let who = demo
            .people
            .iter()
            .find(|p| p.signer.pubky() == s.owner)
            .map(|p| p.name)
            .unwrap_or("?");
        demo.note(&format!(
            "{who}'s file {} does not match its name — attributed to the folder owner",
            s.path.rsplit('/').next().unwrap_or_default()
        ));
    }
    demo.ok(&format!(
        "committed chain unchanged: {} links",
        report.verdict.committed.len()
    ));
    demo.end(false).await;

    // ── Step 10 ───────────────────────────────────────────────────────────────────────────────
    demo.begin("Carol goes quiet");
    demo.offline.push("Carol".into());
    demo.say("Alice adds Cheese and Bob confirms, but Carol has gone. Under unanimity the chain stalls: nothing commits without her vote, and nobody can vote for her.");
    demo.clients[0]
        .propose_body(&Body::Add {
            id: "cheese".into(),
            text: "Cheese".into(),
            qty: None,
        })
        .await
        .expect("propose");
    let acted = demo.everyone_acts().await;
    demo.describe_actions(&acted);
    demo.sweep().await;
    demo.watchdog("Wendy receipted the proposal and Bob's confirmation, with her clock");
    demo.clients[0].sync().await.expect("sync");
    if let Some(Status::Stalled { awaiting, .. }) =
        demo.clients[0].verdict().map(|v| v.status.clone())
    {
        let names: Vec<&str> = awaiting.iter().map(|i| demo.people[*i].name).collect();
        demo.note(&format!("stalled, awaiting {}", names.join(", ")));
    }
    demo.say("The rules allow 5 seconds to respond. Once that has passed on Wendy's clock, the others may close the list as abandoned by Carol. Before Wendy's receipts confirm the silence, such a close is only asserted — the parties' word against hers.");
    ui::countdown(RESPOND_MS / 1000 + 2, "letting Carol's allowance run out").await;
    let close = demo.clients[0]
        .propose_abandoned(&[2])
        .await
        .expect("close");
    demo.ok(&format!(
        "Alice proposed close {} — abandoned by Carol, pending: Cheese",
        close.h16()
    ));
    let acted = demo.everyone_acts().await;
    demo.describe_actions(&acted);
    demo.clients[1]
        .confirm_abandoned(close)
        .await
        .expect("confirm close");
    demo.ok("Bob agreed. A close names its subject; it commits with the non-subjects' confirmations alone.");
    for c in demo.clients.iter_mut().take(2) {
        c.sync().await.expect("sync");
    }
    if let Some(Status::Paused { close, .. }) = demo.clients[0].verdict().map(|v| v.status.clone())
    {
        demo.note(&format!("provisional: the close is {close:?} until a witness quorum has receipts covering it").to_lowercase());
    }
    demo.sweep().await;
    demo.watchdog("Wendy receipted the close and Bob's agreement. Her receipts show Carol silent for longer than the allowance: the close is adjudicated, and final.");
    let acted = demo.everyone_acts().await;
    demo.describe_actions(&acted);
    let v = demo.end(true).await.expect("verdict");
    if let Status::Abandoned { outcome, .. } = &v.status {
        ui::ok(&format!("outcome: {}", outcome.summary));
    }

    // ── Step 11 ───────────────────────────────────────────────────────────────────────────────
    demo.begin("Anyone can check");
    demo.say("A bystander with no seat and no key verifies the list from any one party's folder: every link signed by an established party, every quorum certificate complete, nothing altered, every time question answered by Wendy's receipts. All three folders tell the same story; Carol's tampered copy is noted against her and changes nothing.");
    let observer = PubkyStore::read_only(testnet.sdk().expect("sdk"));
    let mut agreed: Vec<Vec<Hash>> = Vec::new();
    for i in 0..3 {
        let report = verify_from(
            &List,
            &observer,
            &chain,
            (demo.people[i].signer.pubky(), demo.people[i].signer.path()),
        )
        .await
        .expect("verify");
        demo.ok(&format!(
            "from {}'s folder: {} links, status {}, {} suspect file{}",
            demo.people[i].name,
            report.verdict.committed.len(),
            explorer::status_line(&report.verdict.status, &demo.names()),
            report.suspects.len(),
            if report.suspects.len() == 1 { "" } else { "s" }
        ));
        agreed.push(report.verdict.committed_hashes());
    }
    if agreed.windows(2).all(|w| w[0] == w[1]) {
        demo.ok("all three agree on the same committed hashes");
    }
    demo.say("Wendy's folder holds no links (receipts tier), only receipts and her engagement; a mirror-tier watchdog would also be a full copy. Carol, back online, would find the list closed and her Cheese vote late.");
    demo.end(true).await;

    println!(
        "{}The explorer stays up at http://127.0.0.1:{port}/ — Ctrl-C to stop.{}",
        ui::DIM,
        ui::RESET
    );
    if auto {
        return;
    }
    tokio::time::sleep(Duration::from_secs(u64::MAX / 4)).await;
}
