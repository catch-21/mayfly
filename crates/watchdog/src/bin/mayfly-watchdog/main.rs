//! `mayfly-watchdog`: the hosted watchdog of §16.2.1 E.
//!
//! One identity, kept in a file, signed in to one homeserver as one app; an
//! [`Operator`](pubky_mayfly_watchdog::Operator) over that session, swept on a timer, watching
//! its customers' chains for free or against prepaid watch-time (§11.2); `/healthz` and
//! `/status` beside it. Configuration is flags, `MAYFLY_WATCHDOG_*` variables or a TOML file
//! ([`config`]); the loop is [`service`]; the identity file is [`keyfile`].

#![forbid(unsafe_code)]

mod config;
mod health;
mod keyfile;
mod service;

use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use pubky::errors::RequestError;
use pubky::{ClientId, Keypair, Pubky, PubkyHttpClient, PubkySession, PublicKey};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use pubky_mayfly_client::{PubkyStore, SessionSigner, Signer};
use pubky_mayfly_watchdog::{Operator, Terms};

use config::{Cli, Config, Network, Tier};
use health::{Identity, Status};
use service::Service;

type Failure = Box<dyn std::error::Error + Send + Sync>;

/// How often, and how many times, to try to sign in before giving up on startup: a compose
/// stack's homeserver may take a while to come up after the watchdog does.
const STARTUP_ATTEMPTS: u32 = 12;
const STARTUP_RETRY: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = match Config::load(cli) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("mayfly-watchdog: {e}");
            return ExitCode::from(2);
        }
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    match run(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!(error = %e, "mayfly-watchdog stopped");
            ExitCode::FAILURE
        }
    }
}

/// Start everything and sweep until SIGINT or SIGTERM.
async fn run(config: Config) -> Result<(), Failure> {
    let pubky = build_pubky(&config.network)?;
    let (keypair, created) = keyfile::load_or_create(&config.keypair_file)?;
    let me = keypair.public_key().z32();
    if created {
        warn!(
            pubky = %me,
            file = %config.keypair_file.display(),
            "generated a new identity. Back the file up: chains name this pubky in their genesis, and losing it strands every engagement it holds"
        );
    } else {
        info!(pubky = %me, file = %config.keypair_file.display(), "loaded identity");
    }
    let homeserver = PublicKey::try_from(config.homeserver.as_str())?;
    let client_id = ClientId::new(&config.client_id)?;

    let session = connect(
        &pubky,
        &keypair,
        &homeserver,
        config.signup_token.as_deref(),
        &client_id,
    )
    .await?;
    let store = PubkyStore::new(pubky.clone(), session.clone());
    let signer = SessionSigner::from_session(&session).await?;
    let identity = Identity {
        pubky: signer.pubky(),
        kid: signer.kid(),
        client_id: signer.client_id(),
        network: config.network.to_string(),
        homeserver: config.homeserver.clone(),
        path: signer.path(),
    };
    info!(
        pubky = %identity.pubky,
        kid = %identity.kid,
        client_id = %identity.client_id,
        network = %identity.network,
        homeserver = %identity.homeserver,
        folder = %identity.path,
        tier = %config.tier,
        engage_secs = config.engage_secs,
        renew_before_secs = config.renew_before_secs,
        poll_ms = config.poll_ms,
        sweep_secs = config.sweep_secs,
        "watchdog signed in"
    );
    for pubky in &config.free {
        info!(customer = %pubky, credit = "free", "customer");
    }
    for (pubky, secs) in &config.credit {
        info!(customer = %pubky, credit_secs = secs, "customer");
    }
    if config.free.is_empty() && config.credit.is_empty() {
        warn!("no customers configured: nothing will be engaged (--free, --credit)");
    }

    let terms = match config.tier {
        Tier::Receipts => Terms::receipts(0),
        Tier::Mirror => Terms::mirror(0),
    }
    .poll_every(config.poll_ms);
    let mut op = Operator::new(store, signer, terms)
        .engage_for(config.engage_secs)
        .renew_before(config.renew_before_secs);
    for pubky in &config.free {
        op.free(pubky.clone());
    }
    for (pubky, secs) in &config.credit {
        op.credit(pubky.clone(), *secs);
    }

    let status = Status::new(identity, config.sweep_secs);
    let _health = match config.health_addr {
        Some(addr) => {
            let handle = status.serve(addr).await?;
            info!(%addr, "serving /healthz and /status");
            Some(handle)
        }
        None => None,
    };

    Service::new(op, config.customers(), status, config.sweep_secs)
        .run(shutdown())
        .await;
    info!("stopped");
    Ok(())
}

/// The SDK facade for a network.
fn build_pubky(network: &Network) -> Result<Pubky, Failure> {
    Ok(match network {
        Network::Mainnet => Pubky::new()?,
        Network::Testnet => Pubky::testnet()?,
        Network::TestnetHost(host) => {
            let client = PubkyHttpClient::builder().testnet_with_host(host).build()?;
            Pubky::with_client(client)
        }
    })
}

/// A grant session on `homeserver`, whatever state the account is in.
///
/// Sign in first: the ordinary restart. If that fails — no account yet, or a testnet whose
/// DHT has forgotten the `_pubky` record — sign up; an account that already exists answers
/// `409`, in which case only the record needs republishing. Then sign in. Tried a few times
/// with a pause between, so that a homeserver still starting does not fail the service.
async fn connect(
    pubky: &Pubky,
    keypair: &Keypair,
    homeserver: &PublicKey,
    signup_token: Option<&str>,
    client_id: &ClientId,
) -> Result<PubkySession, pubky::Error> {
    let mut attempt = 1;
    loop {
        match connect_once(pubky, keypair.clone(), homeserver, signup_token, client_id).await {
            Ok(session) => return Ok(session),
            Err(e) if attempt < STARTUP_ATTEMPTS => {
                warn!(
                    error = %e,
                    attempt,
                    of = STARTUP_ATTEMPTS,
                    "could not sign in; trying again in {}s",
                    STARTUP_RETRY.as_secs()
                );
                attempt += 1;
                tokio::time::sleep(STARTUP_RETRY).await;
            }
            Err(e) => return Err(e),
        }
    }
}

async fn connect_once(
    pubky: &Pubky,
    keypair: Keypair,
    homeserver: &PublicKey,
    signup_token: Option<&str>,
    client_id: &ClientId,
) -> Result<PubkySession, pubky::Error> {
    let signer = pubky.signer(keypair);
    match signer.signin(client_id.clone()).await {
        Ok(session) => return Ok(session),
        Err(e) => info!(error = %e, "sign-in failed; signing up"),
    }
    match signer.signup(homeserver, signup_token).await {
        Ok(()) => info!(homeserver = %homeserver.z32(), "signed up"),
        Err(e) if is_conflict(&e) => {
            info!("account exists; republishing the homeserver record");
            signer
                .pkdns()
                .publish_homeserver_force(Some(homeserver))
                .await?;
        }
        Err(e) => return Err(e),
    }
    signer.signin(client_id.clone()).await
}

/// The homeserver's `409 Conflict`: the account already exists.
fn is_conflict(e: &pubky::Error) -> bool {
    matches!(
        e,
        pubky::Error::Request(RequestError::Server { status, .. }) if status.as_u16() == 409
    )
}

/// Resolves on SIGINT or SIGTERM.
async fn shutdown() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!(error = %e, "cannot listen for ctrl-c");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    {
        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(term) => term,
                Err(e) => {
                    error!(error = %e, "cannot listen for SIGTERM");
                    ctrl_c.await;
                    return;
                }
            };
        tokio::select! {
            () = ctrl_c => info!("SIGINT"),
            _ = term.recv() => info!("SIGTERM"),
        }
    }
    #[cfg(not(unix))]
    {
        ctrl_c.await;
        info!("SIGINT");
    }
}
