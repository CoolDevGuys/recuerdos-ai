//! `recuerdos-ai user` and `recuerdos-ai key` subcommands.
//!
//! An inbound adapter like any other: it parses arguments, calls exactly
//! one use case, and formats the result. No identity rules live here.

use crate::bootstrap::wiring::Identity;
use crate::identity::domain::scope::Scope;
use crate::shared::error::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum UserCommand {
    /// Create a user.
    Add {
        /// Short unique name, used everywhere else as `--user <handle>`.
        handle: String,
        #[arg(long)]
        email: Option<String>,
    },
    /// List all users.
    List,
}

#[derive(Subcommand)]
pub enum KeyCommand {
    /// Issue an API key. The secret is printed once and never again.
    Issue {
        #[arg(long)]
        user: String,
        /// Comma-separated: read, write, admin.
        #[arg(long, default_value = "read,write")]
        scopes: String,
        /// Label to recognise this key by later.
        #[arg(long, default_value = "default")]
        name: String,
    },
    /// Revoke an API key by its prefix (the visible half, from `key list`).
    Revoke { prefix: String },
    /// List a user's keys, revoked ones included.
    List {
        #[arg(long)]
        user: String,
    },
}

pub fn run_user_command(command: UserCommand, identity: &Identity) -> Result<()> {
    match command {
        UserCommand::Add { handle, email } => {
            let user = identity.user_creator.execute(&handle, email.as_deref())?;
            println!("created user {}", user.handle());
            if let Some(email) = user.email() {
                println!("  email: {email}");
            }
            println!("  id:    {}", user.id());
            Ok(())
        }
        UserCommand::List => {
            let users = identity.users.list()?;
            if users.is_empty() {
                println!("no users yet — create one with `recuerdos-ai user add <handle>`");
                return Ok(());
            }
            for user in users {
                println!(
                    "{:<20} {:<30} created {}",
                    user.handle(),
                    user.email().unwrap_or("-"),
                    user.created_at().format("%Y-%m-%d")
                );
            }
            Ok(())
        }
    }
}

pub fn run_key_command(command: KeyCommand, identity: &Identity) -> Result<()> {
    match command {
        KeyCommand::Issue { user, scopes, name } => {
            let scopes = Scope::parse_list(&scopes)?;
            let issued = identity.api_key_issuer.execute(&user, scopes, &name)?;

            println!(
                "API key created for {} (name: {}, scopes: {})",
                user,
                issued.key.name(),
                Scope::join(issued.key.scopes())
            );
            println!();
            println!("  {}", issued.token.render());
            println!();
            // Stated plainly because it is genuinely unrecoverable: only
            // the hash is stored, so a lost key can only be replaced.
            println!("This is the only time this key is shown. Store it now.");
            Ok(())
        }
        KeyCommand::Revoke { prefix } => {
            identity.api_key_revoker.execute(&prefix)?;
            println!("revoked key {prefix}");
            Ok(())
        }
        KeyCommand::List { user } => {
            let keys = identity.api_key_lister.execute(&user)?;
            if keys.is_empty() {
                println!("{user} has no API keys");
                return Ok(());
            }

            println!(
                "{:<10} {:<16} {:<18} {:<12} STATUS",
                "PREFIX", "NAME", "SCOPES", "LAST USED"
            );
            for key in keys {
                println!(
                    "{:<10} {:<16} {:<18} {:<12} {}",
                    key.prefix(),
                    key.name(),
                    Scope::join(key.scopes()),
                    key.last_used_at()
                        .map(|at| at.format("%Y-%m-%d").to_string())
                        .unwrap_or_else(|| "never".to_string()),
                    match key.revoked_at() {
                        Some(at) => format!("revoked {}", at.format("%Y-%m-%d")),
                        None => "active".to_string(),
                    }
                );
            }
            Ok(())
        }
    }
}
