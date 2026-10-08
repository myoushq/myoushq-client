//! myous command line: the library plus file storage in $MYOUS_HOME
//! (default ~/.myous). Run `myous --help` for the commands.

use std::io::Read;
use std::path::PathBuf;
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
        #[command(flatten)]
        context: ContextArgs,
    },
    /// Join a pairing from a link or code
    Accept {
        code: String,
        /// Seconds to wait for the other side
        #[arg(long, default_value_t = 60.0)]
        wait: f64,
        #[command(flatten)]
        context: ContextArgs,
    },
    /// Show or set how your owner knows a contact and what you may share with it
    Context {
        name: String,
        #[command(flatten)]
        context: ContextArgs,
        #[arg(long)]
        json: bool,
    },
    /// Send a message to a paired contact (text "-" reads stdin)
    Send {
        to: String,
        #[arg(required = true)]
        text: Vec<String>,
    },
    /// Send a file to a paired contact (encrypted end to end; the hub stores a blob it can't read)
    SendFile { to: String, path: PathBuf },
    /// Download and decrypt a received file (the latest, or by history seq)
    Fetch {
        /// History seq of the file entry (see `myous inbox --json`)
        seq: Option<u64>,
        /// The most recent received file
        #[arg(long)]
        latest: bool,
        /// Directory to write into (default $MYOUS_HOME/files)
        #[arg(long)]
        to: Option<PathBuf>,
    },
    /// Fetch, then show new messages and other items, and mark them read
    Inbox {
        #[arg(long)]
        peek: bool,
        /// Don't fetch; show only what's already stored
        #[arg(long)]
        local: bool,
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
        Command::Invite { json, wait, context } => {
            let inv = agent.invite(context.into()).await?;
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
        Command::Context { name, context, json } => {
            let c = agent.set_context(&name, &context.into())?;
            if json {
                println!("{}", json!({"alias": c.alias, "relationship": c.relationship, "sharing": c.sharing}));
            } else {
                println!("{}: relationship {}; may share: {}", c.alias,
                    c.relationship.as_deref().unwrap_or("(not set)"),
                    c.sharing.as_deref().unwrap_or("(not set: share nothing personal)"));
            }
        }
        Command::Accept { code, wait, context } => match agent.accept(&code, Duration::from_secs_f64(wait), context.into()).await? {
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
        Command::SendFile { to, path } => {
            let entry = agent.send_file(&to, &path, vec![]).await?;
            println!("sent {} ({} bytes) to {}", entry["name"].as_str().unwrap_or(""), entry["size"], entry["alias"].as_str().unwrap_or(&to));
        }
        Command::Fetch { seq, latest, to } => {
            let files: Vec<Value> = st.read_history_or_empty().into_iter()
                .filter(|e| e["type"] == "file" && e["direction"] == "in").collect();
            let entry = match (seq, latest) {
                (Some(seq), _) => files.iter().find(|e| e["seq"] == seq).cloned(),
                (None, true) => files.last().cloned(),
                _ => bail!("say which file: `myous fetch SEQ` or `myous fetch --latest`"),
            };
            let Some(entry) = entry else { bail!("no such received file") };
            let dir = to.unwrap_or_else(|| st.home.join("files"));
            let path = agent.fetch(&entry, &dir).await?;
            println!("{}", path.display());
        }
        Command::Inbox { peek, local, json } => {
            if !local {
                if let Err(e) = agent.poll().await {
                    eprintln!("warning: couldn't fetch new items ({e}); showing what's stored");
                }
            }
            let entries = agent.unread(!peek)?;
            if json {
                // The key and nonce stay in the history; they're not for display.
                let shown: Vec<Value> = entries.iter().map(|e| {
                    let mut e = e.clone();
                    if let Some(o) = e.as_object_mut() {
                        o.remove("key");
                        o.remove("nonce");
                    }
                    e
                }).collect();
                println!("{}", serde_json::to_string_pretty(&shown)?);
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
                    println!("{:<20} {:<9} {:<10} {}", c.alias, c.status, c.relationship.as_deref().unwrap_or("-"), c.npub);
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
                "pending_pairings": if has_identity {
                    agent.pending_pairings()?.iter().map(describe_pending).collect::<Vec<_>>()
                } else {
                    vec![]
                },
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

/// What a pairing in progress is waiting for.
fn describe_pending(p: &myous::pairing::Pending) -> String {
    let waiting = match (p.stage.as_str(), p.role.as_str()) {
        ("wait_pake", "a") => "waiting for the other agent to join",
        ("wait_pake", _) => "waiting for the inviting agent to answer",
        _ => "waiting for the other agent's details",
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{}: {waiting}, expires in {} min", p.nameplate, p.expires_at.saturating_sub(now) / 60)
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
            (Some("message"), _) => {
                println!("[{when}] {alias}: {text}");
                println!("{}", context_line(e));
            }
            (Some("file"), Some("out")) => println!("[{when}] me -> {alias}: file {} ({} bytes)", e["name"].as_str().unwrap_or(""), e["size"]),
            (Some("file"), _) => {
                println!("[{when}] file from {alias}: {} ({} bytes, {}); fetch it with `myous fetch {}`",
                    e["name"].as_str().unwrap_or(""), e["size"], e["mime"].as_str().unwrap_or(""), e["seq"]);
                println!("{}", context_line(e));
            }
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
    fn read_history_or_empty(&self) -> Vec<Value>;
}

impl GetOrEmpty for FileStorage {
    fn storage_get(&self, name: &str) -> Value {
        use myous::Storage;
        self.get(name).ok().flatten().unwrap_or(json!({}))
    }

    fn read_history_or_empty(&self) -> Vec<Value> {
        use myous::Storage;
        self.read_history().unwrap_or_default()
    }
}

/// Relationship context options, shared by invite, accept and context.
#[derive(clap::Args, Debug)]
struct ContextArgs {
    /// How your owner knows this contact: family, friend, colleague, business, service or other
    #[arg(long)]
    relationship: Option<String>,
    /// Your owner's guidance on what you may share with this contact
    #[arg(long)]
    sharing: Option<String>,
}

impl From<ContextArgs> for myous::contacts::ContactContext {
    fn from(a: ContextArgs) -> Self {
        Self { relationship: a.relationship, sharing: a.sharing }
    }
}

/// How the owner knows the sender of an incoming message and what may be
/// shared, so the agent has it when it answers.
fn context_line(e: &Value) -> String {
    let (relationship, sharing) = (e["relationship"].as_str(), e["sharing"].as_str());
    if relationship.is_none() && sharing.is_none() {
        return format!("    (relationship not set: until your owner tells you, share nothing personal; record it with \
            myous context {:?} --relationship ... --sharing \"...\")", e["alias"].as_str().unwrap_or(""));
    }
    let mut line = relationship.unwrap_or("relationship not set").to_string();
    if let Some(s) = sharing {
        line += &format!("; may share: {s}");
    }
    format!("    ({line})")
}
