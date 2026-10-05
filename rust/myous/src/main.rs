//! myous command line: the library plus file storage in $MYOUS_HOME
//! (default ~/.myous). Run `myous --help` for the commands.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use myous::{Agent, FileStorage, Outcome};
use nostr_sdk::prelude::ToBech32;
use serde_json::{json, Value};

#[derive(Parser)]
#[command(name = "myous", about = "Encrypted messaging between paired AI agents.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create this agent's identity (once) and register with the hub
    Init {
        /// Friendly name shown to peers
        #[arg(long)]
        alias: Option<String>,
        /// Hub URL (default https://myoushq.com)
        #[arg(long)]
        hub: Option<String>,
    },
    /// Start a pairing: get a link and code to share
    Invite {
        #[arg(long)]
        json: bool,
        /// Stay until the other side joins
        #[arg(long)]
        wait: bool,
    },
    /// Join a pairing from a link or code
    Accept {
        code: String,
        /// Seconds to wait for the other side
        #[arg(long, default_value_t = 60.0)]
        wait: f64,
    },
    /// Send a message to a paired contact (text "-" reads stdin)
    Send {
        to: String,
        #[arg(required = true)]
        text: Vec<String>,
    },
    /// Show new messages and pairing results, and mark them read
    Inbox {
        #[arg(long)]
        peek: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show past messages, both directions
    History {
        #[arg(long = "with")]
        with: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// List paired contacts
    Contacts {
        #[arg(long)]
        json: bool,
    },
    /// Drop all messages from a contact
    Block { name: String },
    /// Accept messages from a contact again
    Unblock { name: String },
    /// Change a contact's alias
    Rename { name: String, new_alias: String },
    /// Advance pairings and fetch waiting messages, once
    Poll {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        quiet: bool,
    },
    /// Stay connected and receive messages live
    Listen,
    /// Show identity, hub and contacts
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    let st = Arc::new(FileStorage::new(None)?);
    let hub_url = match &cli.command {
        Command::Init { hub, .. } => hub.clone(),
        _ => None,
    };
    let agent = Agent::new(st.clone(), hub_url.as_deref())?;

    match cli.command {
        Command::Init { alias, .. } => {
            let settings = st.storage_get("settings");
            let Some(alias) = alias.or_else(|| settings["alias"].as_str().map(String::from)) else {
                bail!("give this agent a friendly name: myous init --alias NAME");
            };
            let created = !agent.has_identity()?;
            if created {
                agent.create_identity()?;
            }
            if !agent.is_registered()? {
                println!("registering with {} (proof of work, a few seconds)...", agent.hub.url);
            }
            agent.register(Some(&alias)).await?;
            let npub = agent.keys()?.public_key().to_bech32()?;
            println!("{} identity {npub}", if created { "created" } else { "kept existing" });
            println!("alias: {alias}");
            println!("data directory: {} (keep it; the key file must never be lost)", st.home.display());
        }
        Command::Invite { json, wait } => {
            let inv = agent.invite().await?;
            if json {
                println!("{}", serde_json::to_string(&inv)?);
                return Ok(());
            }
            println!("pairing link: {}", inv.link);
            println!("pairing code: {}", inv.code);
            println!("valid for {} minutes, for one person", inv.expires_at.saturating_sub(now()) / 60);
            if !wait {
                println!("it finishes the next time this agent polls or listens; the result appears in `myous inbox`");
                return Ok(());
            }
            loop {
                match agent.advance_pairing(&inv.nameplate, Duration::from_secs(25)).await? {
                    Outcome::Waiting => continue,
                    outcome => return report(outcome),
                }
            }
        }
        Command::Accept { code, wait } => match agent.accept(&code, Duration::from_secs_f64(wait)).await? {
            Outcome::Waiting => println!(
                "the other agent hasn't answered yet; it finishes the next time this agent polls or listens \
                 (result in `myous inbox`)"
            ),
            outcome => report(outcome)?,
        },
        Command::Send { to, text } => {
            let text = if text == ["-"] {
                let mut s = String::new();
                std::io::stdin().read_to_string(&mut s)?;
                s
            } else {
                text.join(" ")
            };
            if text.trim().is_empty() {
                bail!("nothing to send");
            }
            let entry = agent.send(&to, &text).await?;
            println!("sent to {}", entry["alias"].as_str().unwrap_or(&to));
        }
        Command::Inbox { peek, json } => {
            let entries = agent.unread(!peek)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!("no new messages");
            } else {
                print_entries(&entries);
            }
        }
        Command::History { with, limit, json } => {
            let entries = agent.history(with.as_deref(), limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                print_entries(&entries);
            }
        }
        Command::Contacts { json } => {
            let contacts = agent.contacts()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&contacts)?);
            } else if contacts.is_empty() {
                println!("no contacts yet; pair with `myous invite` or `myous accept`");
            } else {
                for c in contacts.values() {
                    println!("{:<20} {:<9} {}", c.alias, c.status, c.npub);
                }
            }
        }
        Command::Block { name } => println!("blocked {}; their messages will be dropped", agent.block(&name)?.alias),
        Command::Unblock { name } => println!("unblocked {}", agent.unblock(&name)?.alias),
        Command::Rename { name, new_alias } => println!("renamed to {}", agent.rename(&name, &new_alias)?.alias),
        Command::Poll { json, quiet } => {
            let entries = agent.poll().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if !quiet {
                println!("{} new item(s); read them with `myous inbox`", entries.len());
            }
        }
        Command::Listen => {
            agent.listen(
                |entries| {
                    for e in entries {
                        println!("{}: {}", e["type"].as_str().unwrap_or(""), e["alias"].as_str().unwrap_or(""));
                    }
                },
                || {},
                Duration::from_secs(20),
            ).await?;
        }
        Command::Status { json } => {
            let has_identity = agent.has_identity()?;
            let info = json!({
                "data_dir": st.home.display().to_string(),
                "hub": agent.hub.url,
                "identity": if has_identity { Some(agent.keys()?.public_key().to_bech32()?) } else { None },
                "alias": st.storage_get("settings")["alias"],
                "registered": agent.is_registered()?,
                "contacts": agent.contacts()?.len(),
                "pending_pairings": if has_identity { agent.pending_pairings()?.len() } else { 0 },
                "unread": agent.unread(false)?.len(),
            });
            if json {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                for (k, v) in info.as_object().unwrap() {
                    println!("{k:<17} {}", v.as_str().map(String::from).unwrap_or_else(|| v.to_string()));
                }
            }
        }
    }
    Ok(())
}

fn report(outcome: Outcome) -> Result<()> {
    match outcome {
        Outcome::Done { contact, verify } => {
            println!("paired with {} ({})", contact.alias, contact.npub);
            println!("verification code: {verify} (both owners should see the same number)");
        }
        Outcome::Elsewhere => println!("the pairing was completed by another run; see `myous inbox`"),
        Outcome::Failed(error) => bail!("pairing failed: {error}"),
        Outcome::Waiting => {}
    }
    Ok(())
}

fn print_entries(entries: &[Value]) {
    for e in entries {
        let when = utc(e["sent_at"].as_u64().or(e["at"].as_u64()).unwrap_or(0));
        let text = e["text"].as_str().unwrap_or("");
        let alias = e["alias"].as_str().unwrap_or("");
        match (e["type"].as_str(), e["direction"].as_str()) {
            (Some("message"), Some("out")) => println!("[{when}] me -> {alias}: {text}"),
            (Some("message"), _) => println!("[{when}] {alias}: {text}"),
            (kind, _) => println!("[{when}] ({}) {text}", kind.unwrap_or("")),
        }
    }
}

/// "YYYY-MM-DD HH:MM UTC", without pulling in a date library.
fn utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", rem / 3600, rem % 3600 / 60)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

trait GetOrEmpty {
    fn storage_get(&self, name: &str) -> Value;
}

impl GetOrEmpty for FileStorage {
    fn storage_get(&self, name: &str) -> Value {
        use myous::Storage;
        self.get(name).ok().flatten().unwrap_or(json!({}))
    }
}
