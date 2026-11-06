// SPDX-License-Identifier: Apache-2.0
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(version, about = "Sidecar — a RAW editor beside Immich")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Public HTTP api, web app, sidecar writer.
    Api {
        #[arg(long, default_value = "/etc/sidecar.toml")]
        config: PathBuf,
    },
    /// RAW decoding and GPU rendering, internal network only.
    Worker {
        #[arg(long, default_value = "0.0.0.0:8081")]
        listen: String,
        #[arg(long, default_value = "/cache")]
        cache: PathBuf,
    },
    /// Local accounts, from the server itself: the way back in when nobody remembers a password.
    Account {
        #[arg(long, default_value = "/etc/sidecar.toml")]
        config: PathBuf,
        #[command(subcommand)]
        action: AccountAction,
    },
    /// Development: api and worker in one process.
    Dev {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = "/cache/sidecar-dev")]
        cache: PathBuf,
    },
}

#[derive(Subcommand)]
enum AccountAction {
    /// The accounts there are.
    List,
    /// Set a password, signing that account out everywhere. An account that doesn't exist yet is created, as
    /// an administrator. Asks for the password; piped in, it reads one line.
    SetPassword { username: String },
}

async fn account(config: PathBuf, action: AccountAction) -> Result<()> {
    let config = sidecar_api::config::Config::load(&config)?;
    let store = sidecar_api::store::Store::new(config.server.state_dir.clone())?;
    match action {
        AccountAction::List => {
            for a in sidecar_api::accounts::all(&store).await {
                let role = if a.is_admin { "administrator" } else { "" };
                println!("{:<24} {:<32} {role}", a.username, a.name);
            }
        }
        AccountAction::SetPassword { username } => {
            let password = if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                let first = rpassword::prompt_password(format!("New password for {username}: "))?;
                if rpassword::prompt_password("Again: ")? != first {
                    anyhow::bail!("the two passwords differ; nothing was changed");
                }
                first
            } else {
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                line.trim_end_matches(['\r', '\n']).to_string()
            };
            let a = sidecar_api::accounts::set_password(&store, &username, &password).await?;
            println!(
                "{}'s password is set{}.",
                a.username,
                if a.is_admin { " (administrator)" } else { "" }
            );
        }
    }
    Ok(())
}

async fn serve(listen: &str, router: axum::Router) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!("listening on {listen}");
    axum::serve(listener, router).await?;
    Ok(())
}

fn api_app(config: PathBuf) -> Result<Arc<sidecar_api::App>> {
    sidecar_api::App::new(sidecar_api::config::Config::load(&config)?)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    match Cli::parse().command {
        Command::Api { config } => {
            let app = api_app(config)?;
            let listen = app.config.server.listen.clone();
            serve(&listen, sidecar_api::router(app)).await
        }
        Command::Worker { listen, cache } => {
            serve(
                &listen,
                sidecar_worker::router(sidecar_worker::Worker::new(cache)?),
            )
            .await
        }
        Command::Account { config, action } => account(config, action).await,
        Command::Dev { config, cache } => {
            let app = api_app(config)?;
            let worker = sidecar_worker::Worker::new(cache)?;
            let worker_listen = app
                .config
                .server
                .worker_url
                .trim_start_matches("http://")
                .to_string();
            let api_listen = app.config.server.listen.clone();
            tokio::try_join!(
                serve(&worker_listen, sidecar_worker::router(worker)),
                serve(&api_listen, sidecar_api::router(app))
            )?;
            Ok(())
        }
    }
}
