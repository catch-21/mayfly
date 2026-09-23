//! Configuration (§16.2.1 E): flags, their environment variables, and an optional TOML file.
//!
//! Precedence, per setting: the flag or its `MAYFLY_WATCHMAN_*` variable, then the file named
//! by `--config`, then the default. Lists (`free`, `credit`) are the union of every source.

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use clap::Parser;
use serde::Deserialize;

/// The `client_id` the watchman publishes under when none is given: it decides the
/// `/pub/<client_id>/mayfly/` folder every engagement and receipt is written to.
pub const DEFAULT_CLIENT_ID: &str = "watchman.mayfly.example";
/// Where the identity lives when `--keypair-file` is not given.
pub const DEFAULT_KEYPAIR_FILE: &str = "/var/lib/mayfly-watchman/keypair";
/// Engagement length: one day.
pub const DEFAULT_ENGAGE_SECS: u64 = 86_400;
/// Renew an engagement within an hour of its `until`.
pub const DEFAULT_RENEW_BEFORE_SECS: u64 = 3_600;
/// The `poll_ms` published in every engagement (§11.2).
pub const DEFAULT_POLL_MS: u64 = 5_000;
/// Seconds between `Operator::sweep()` calls.
pub const DEFAULT_SWEEP_SECS: u64 = 15;

/// Which Pubky network the watchman joins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Network {
    /// The Mainline DHT and the default pkarr relays.
    Mainnet,
    /// A local testnet on `localhost`: `StaticTestnet`'s well-known ports.
    Testnet,
    /// A testnet whose DHT bootstrap node and pkarr relay are on `host`. Only those two move;
    /// the homeserver is reached at whatever its own pkarr record advertises.
    TestnetHost(String),
}

impl FromStr for Network {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "mainnet" => Ok(Self::Mainnet),
            "testnet" => Ok(Self::Testnet),
            other => match other.strip_prefix("testnet:") {
                Some(host) if !host.is_empty() && !host.contains('/') => {
                    Ok(Self::TestnetHost(host.to_string()))
                }
                _ => Err(format!(
                    "network {s:?}: expected `mainnet`, `testnet` or `testnet:<host>`"
                )),
            },
        }
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mainnet => f.write_str("mainnet"),
            Self::Testnet => f.write_str("testnet"),
            Self::TestnetHost(host) => write!(f, "testnet:{host}"),
        }
    }
}

/// The service tier of every engagement (§11.2, §11.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// One signed receipt per observed record.
    Receipts,
    /// Receipts and byte-for-byte copies.
    Mirror,
}

impl FromStr for Tier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "receipts" => Ok(Self::Receipts),
            "mirror" => Ok(Self::Mirror),
            _ => Err(format!("tier {s:?}: expected `receipts` or `mirror`")),
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Receipts => f.write_str("receipts"),
            Self::Mirror => f.write_str("mirror"),
        }
    }
}

/// `<pubky>=<secs>`: a prepaid customer and their watch-time.
pub fn parse_credit(s: &str) -> Result<(String, u64), String> {
    let (pubky, secs) = s
        .split_once('=')
        .ok_or_else(|| format!("credit {s:?}: expected `<pubky>=<secs>`"))?;
    let pubky = pubky.trim();
    check_pubky(pubky)?;
    let secs = secs
        .trim()
        .parse::<u64>()
        .map_err(|e| format!("credit {s:?}: seconds: {e}"))?;
    Ok((pubky.to_string(), secs))
}

/// A pubky is 52 characters of z-base-32.
fn check_pubky(s: &str) -> Result<(), String> {
    pubky_common::crypto::PublicKey::try_from(s)
        .map(|_| ())
        .map_err(|e| format!("pubky {s:?}: {e}"))
}

/// The command line. Every flag has an environment variable; `--config` names a TOML file
/// that fills in whatever the flags and the environment leave unset.
#[derive(Debug, Default, Parser)]
#[command(
    name = "mayfly-watchman",
    version,
    about = "A Mayfly watchman: engages on customers' chains and receipts every record it observes (MAYFLY.md §11)."
)]
pub struct Cli {
    /// A TOML file with the same keys as these flags, in snake case.
    #[arg(long, env = "MAYFLY_WATCHMAN_CONFIG", value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// `mainnet` (default), `testnet` or `testnet:<host>`.
    #[arg(long, env = "MAYFLY_WATCHMAN_NETWORK", value_name = "NETWORK")]
    pub network: Option<Network>,
    /// The homeserver the watchman signs up on, as a z-base-32 pubky.
    #[arg(long, env = "MAYFLY_WATCHMAN_HOMESERVER", value_name = "PUBKY")]
    pub homeserver: Option<String>,
    /// A signup token, if the homeserver requires one. Used on first signup only.
    #[arg(
        long,
        env = "MAYFLY_WATCHMAN_SIGNUP_TOKEN",
        value_name = "TOKEN",
        hide_env_values = true
    )]
    pub signup_token: Option<String>,
    /// The app the watchman signs in as; decides its `/pub/<client_id>/mayfly/` folder.
    #[arg(long, env = "MAYFLY_WATCHMAN_CLIENT_ID", value_name = "ID")]
    pub client_id: Option<String>,
    /// The identity file: 32 secret bytes as hex. Generated on first run if absent.
    #[arg(long, env = "MAYFLY_WATCHMAN_KEYPAIR_FILE", value_name = "PATH")]
    pub keypair_file: Option<PathBuf>,
    /// A customer watched for free. Repeatable; comma-separated in the environment.
    #[arg(
        long,
        env = "MAYFLY_WATCHMAN_FREE",
        value_name = "PUBKY",
        value_delimiter = ','
    )]
    pub free: Vec<String>,
    /// A prepaid customer, `<pubky>=<secs>`. Repeatable; comma-separated in the environment.
    #[arg(
        long,
        env = "MAYFLY_WATCHMAN_CREDIT",
        value_name = "PUBKY=SECS",
        value_delimiter = ','
    )]
    pub credit: Vec<String>,
    /// How long each engagement runs before it is renewed, in seconds.
    #[arg(long, env = "MAYFLY_WATCHMAN_ENGAGE_SECS", value_name = "SECS")]
    pub engage_secs: Option<u64>,
    /// How long before `until` an engagement is renewed, in seconds.
    #[arg(long, env = "MAYFLY_WATCHMAN_RENEW_BEFORE_SECS", value_name = "SECS")]
    pub renew_before_secs: Option<u64>,
    /// The `poll_ms` every engagement publishes: the clock tolerance verifiers allow.
    #[arg(long, env = "MAYFLY_WATCHMAN_POLL_MS", value_name = "MS")]
    pub poll_ms: Option<u64>,
    /// Seconds between sweeps of every customer's `/pub/` and every watched chain.
    #[arg(long, env = "MAYFLY_WATCHMAN_SWEEP_SECS", value_name = "SECS")]
    pub sweep_secs: Option<u64>,
    /// `receipts` (default) or `mirror`.
    #[arg(long, env = "MAYFLY_WATCHMAN_TIER", value_name = "TIER")]
    pub tier: Option<Tier>,
    /// Serve `GET /healthz` and `GET /status` here, e.g. `127.0.0.1:8790`.
    #[arg(long, env = "MAYFLY_WATCHMAN_HEALTH_ADDR", value_name = "ADDR")]
    pub health_addr: Option<SocketAddr>,
}

/// The TOML file. Every key optional; `credit` is a table of pubky to seconds.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    /// See [`Cli::network`].
    pub network: Option<String>,
    /// See [`Cli::homeserver`].
    pub homeserver: Option<String>,
    /// See [`Cli::signup_token`].
    pub signup_token: Option<String>,
    /// See [`Cli::client_id`].
    pub client_id: Option<String>,
    /// See [`Cli::keypair_file`].
    pub keypair_file: Option<PathBuf>,
    /// See [`Cli::free`].
    #[serde(default)]
    pub free: Vec<String>,
    /// See [`Cli::credit`].
    #[serde(default)]
    pub credit: BTreeMap<String, u64>,
    /// See [`Cli::engage_secs`].
    pub engage_secs: Option<u64>,
    /// See [`Cli::renew_before_secs`].
    pub renew_before_secs: Option<u64>,
    /// See [`Cli::poll_ms`].
    pub poll_ms: Option<u64>,
    /// See [`Cli::sweep_secs`].
    pub sweep_secs: Option<u64>,
    /// See [`Cli::tier`].
    pub tier: Option<Tier>,
    /// See [`Cli::health_addr`].
    pub health_addr: Option<SocketAddr>,
}

impl FileConfig {
    /// Parse a TOML file.
    pub fn read(path: &Path) -> Result<Self, String> {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("config {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("config {}: {e}", path.display()))
    }

    /// Parse TOML text.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

/// Everything the service needs, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The network.
    pub network: Network,
    /// The homeserver pubky, z-base-32.
    pub homeserver: String,
    /// The signup token, if any.
    pub signup_token: Option<String>,
    /// The app the watchman signs in as.
    pub client_id: String,
    /// The identity file.
    pub keypair_file: PathBuf,
    /// Customers watched for free.
    pub free: Vec<String>,
    /// Prepaid customers and their watch-time in seconds.
    pub credit: BTreeMap<String, u64>,
    /// Engagement length in seconds.
    pub engage_secs: u64,
    /// Renewal margin in seconds.
    pub renew_before_secs: u64,
    /// The published `poll_ms`.
    pub poll_ms: u64,
    /// Seconds between sweeps.
    pub sweep_secs: u64,
    /// The service tier.
    pub tier: Tier,
    /// Where to serve health and status, if anywhere.
    pub health_addr: Option<SocketAddr>,
}

/// A value that is set and non-empty. An empty environment variable counts as unset.
fn given(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

impl Config {
    /// Read the file named by `--config`, if any, and resolve.
    pub fn load(cli: Cli) -> Result<Self, String> {
        let file = match &cli.config {
            Some(path) => FileConfig::read(path)?,
            None => FileConfig::default(),
        };
        Self::resolve(cli, file)
    }

    /// Resolve flags and environment over the file over the defaults.
    pub fn resolve(cli: Cli, file: FileConfig) -> Result<Self, String> {
        let network = match cli.network {
            Some(n) => n,
            None => match given(file.network) {
                Some(n) => n.parse()?,
                None => Network::Mainnet,
            },
        };
        let homeserver = given(cli.homeserver)
            .or(given(file.homeserver))
            .ok_or("--homeserver <pubky> is required (MAYFLY_WATCHMAN_HOMESERVER)")?;
        check_pubky(&homeserver).map_err(|e| format!("homeserver: {e}"))?;
        let client_id = given(cli.client_id)
            .or(given(file.client_id))
            .unwrap_or_else(|| DEFAULT_CLIENT_ID.to_string());

        let mut free: Vec<String> = Vec::new();
        for pubky in cli.free.into_iter().chain(file.free) {
            let pubky = pubky.trim();
            if pubky.is_empty() {
                continue;
            }
            check_pubky(pubky).map_err(|e| format!("free: {e}"))?;
            if !free.iter().any(|f| f == pubky) {
                free.push(pubky.to_string());
            }
        }
        let mut credit: BTreeMap<String, u64> = BTreeMap::new();
        for entry in cli.credit {
            if entry.trim().is_empty() {
                continue;
            }
            let (pubky, secs) = parse_credit(&entry)?;
            *credit.entry(pubky).or_default() += secs;
        }
        for (pubky, secs) in file.credit {
            check_pubky(&pubky).map_err(|e| format!("credit: {e}"))?;
            *credit.entry(pubky).or_default() += secs;
        }

        let engage_secs = cli
            .engage_secs
            .or(file.engage_secs)
            .unwrap_or(DEFAULT_ENGAGE_SECS);
        let renew_before_secs = cli
            .renew_before_secs
            .or(file.renew_before_secs)
            .unwrap_or(DEFAULT_RENEW_BEFORE_SECS);
        let poll_ms = cli.poll_ms.or(file.poll_ms).unwrap_or(DEFAULT_POLL_MS);
        let sweep_secs = cli
            .sweep_secs
            .or(file.sweep_secs)
            .unwrap_or(DEFAULT_SWEEP_SECS);
        if engage_secs == 0 || poll_ms == 0 || sweep_secs == 0 {
            return Err("--engage-secs, --poll-ms and --sweep-secs must be at least 1".into());
        }
        if renew_before_secs >= engage_secs {
            return Err(format!(
                "--renew-before-secs ({renew_before_secs}) must be less than --engage-secs ({engage_secs})"
            ));
        }

        Ok(Self {
            network,
            homeserver,
            signup_token: given(cli.signup_token).or(given(file.signup_token)),
            client_id,
            keypair_file: cli
                .keypair_file
                .or(file.keypair_file)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_KEYPAIR_FILE)),
            free,
            credit,
            engage_secs,
            renew_before_secs,
            poll_ms,
            sweep_secs,
            tier: cli.tier.or(file.tier).unwrap_or(Tier::Receipts),
            health_addr: cli.health_addr.or(file.health_addr),
        })
    }

    /// Every customer, free first, then prepaid by pubky.
    pub fn customers(&self) -> Vec<String> {
        let mut out = self.free.clone();
        for pubky in self.credit.keys() {
            if !out.contains(pubky) {
                out.push(pubky.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HS: &str = "8pinxxgqs41n4aididenw5apqp1urfmzdztr8jt4abrkdn435ewo";

    fn pubky(seed: u8) -> String {
        pubky_common::crypto::Keypair::from_secret(&[seed; 32])
            .public_key()
            .z32()
    }

    /// Parse a command line with no environment involved.
    fn cli(args: &[&str]) -> Cli {
        let mut argv = vec!["mayfly-watchman"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).unwrap()
    }

    #[test]
    fn networks_parse_including_a_testnet_host() {
        assert_eq!("mainnet".parse::<Network>().unwrap(), Network::Mainnet);
        assert_eq!("testnet".parse::<Network>().unwrap(), Network::Testnet);
        assert_eq!(
            "testnet:testnet".parse::<Network>().unwrap(),
            Network::TestnetHost("testnet".into())
        );
        assert_eq!(
            "testnet:192.168.1.50"
                .parse::<Network>()
                .unwrap()
                .to_string(),
            "testnet:192.168.1.50"
        );
        assert!("testnet:".parse::<Network>().is_err());
        assert!("devnet".parse::<Network>().is_err());
    }

    #[test]
    fn credit_flags_and_comma_separated_free_lists_resolve() {
        let (a, b, c) = (pubky(1), pubky(2), pubky(3));
        let free = format!("{a},{b}");
        let credit = format!("{c}=10");
        let again = format!("{c}=5");
        let c = cli(&[
            "--homeserver",
            HS,
            "--free",
            &free,
            "--credit",
            &credit,
            "--credit",
            &again,
            "--network",
            "testnet:testnet",
        ]);
        let config = Config::resolve(c, FileConfig::default()).unwrap();
        assert_eq!(config.free, vec![a.clone(), b.clone()]);
        assert_eq!(config.credit.get(&pubky(3)), Some(&15), "credit adds up");
        assert_eq!(config.network, Network::TestnetHost("testnet".into()));
        assert_eq!(config.client_id, DEFAULT_CLIENT_ID);
        assert_eq!(config.keypair_file, PathBuf::from(DEFAULT_KEYPAIR_FILE));
        assert_eq!(config.engage_secs, DEFAULT_ENGAGE_SECS);
        assert_eq!(config.tier, Tier::Receipts);
        assert_eq!(config.customers(), vec![a, b, pubky(3)]);
    }

    #[test]
    fn bad_values_are_refused() {
        assert!(parse_credit("nobody=10").is_err(), "not a pubky");
        assert!(parse_credit(&format!("{}=ten", pubky(1))).is_err());
        assert!(parse_credit(&pubky(1)).is_err(), "no `=`");
        assert!(
            Config::resolve(cli(&[]), FileConfig::default()).is_err(),
            "homeserver is required"
        );
        assert!(Config::resolve(
            cli(&[
                "--homeserver",
                HS,
                "--engage-secs",
                "60",
                "--renew-before-secs",
                "60"
            ]),
            FileConfig::default()
        )
        .is_err());
        assert!(Config::resolve(
            cli(&["--homeserver", HS, "--sweep-secs", "0"]),
            FileConfig::default()
        )
        .is_err());
    }

    #[test]
    fn the_file_fills_in_what_flags_leave_unset() {
        let text = format!(
            r#"
network = "testnet"
homeserver = "{HS}"
client_id = "dog.example"
keypair_file = "/tmp/dog.key"
free = ["{}"]
tier = "mirror"
sweep_secs = 30
health_addr = "0.0.0.0:8790"

[credit]
"{}" = 3600
"#,
            pubky(1),
            pubky(2)
        );
        let file = FileConfig::parse(&text).unwrap();
        let config = Config::resolve(cli(&["--sweep-secs", "5"]), file).unwrap();
        assert_eq!(config.network, Network::Testnet);
        assert_eq!(config.homeserver, HS);
        assert_eq!(config.client_id, "dog.example");
        assert_eq!(config.keypair_file, PathBuf::from("/tmp/dog.key"));
        assert_eq!(config.free, vec![pubky(1)]);
        assert_eq!(config.credit.get(&pubky(2)), Some(&3600));
        assert_eq!(config.tier, Tier::Mirror);
        assert_eq!(config.sweep_secs, 5, "the flag wins");
        assert_eq!(
            config.health_addr,
            Some("0.0.0.0:8790".parse::<SocketAddr>().unwrap())
        );
        assert!(FileConfig::parse("homeserver = 1").is_err(), "wrong type");
        assert!(
            FileConfig::parse("homeservre = \"x\"").is_err(),
            "unknown keys are refused, so a typo is not a silent default"
        );
    }
}
