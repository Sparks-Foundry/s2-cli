use axum::{routing::post, Json, Router};
use clap::{Parser, Subcommand};
use colored::*;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// Control-plane schema
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ControlPlane {
    brokers: Vec<Broker>,
    live_runtimes: Vec<Runtime>,
    #[serde(default)]
    scaffolded_runtimes: Vec<Runtime>,
}

#[derive(Debug, Deserialize, Clone)]
struct Broker {
    name: String,
    port: u16,
    language: String,
    status: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    gaps: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct Runtime {
    name: String,
    port: u16,
    language: String,
    #[serde(default)]
    gaps: Vec<String>,
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "s2", about = "S2Forge fleet management CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show status for all services (default)
    Status,
    /// Show status for brokers only
    Brokers,
    /// Show status for runtimes only
    Runtimes,
    /// Ping the /health endpoint for a named service
    Health {
        /// Service name as it appears in control-plane.json
        name: String,
    },
    /// List all services with non-empty gaps[] entries
    Gaps,
    /// Start a local webhook listener for Railway deployment events
    Watch {
        /// Port to listen on
        #[arg(short, long, default_value_t = 4000)]
        port: u16,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env.local walking up from CWD
    let mut current = env::current_dir()?;
    loop {
        let candidate = current.join(".env.local");
        if candidate.exists() {
            let _ = dotenvy::from_path(&candidate);
            break;
        }
        if !current.pop() {
            break;
        }
    }

    let cli = Cli::parse();

    match cli.command {
        Commands::Status => cmd_status().await?,
        Commands::Brokers => cmd_brokers().await?,
        Commands::Runtimes => cmd_runtimes().await?,
        Commands::Health { name } => cmd_health(&name).await?,
        Commands::Gaps => cmd_gaps().await?,
        Commands::Watch { port } => cmd_watch(port).await?,
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Control-plane loader
// ---------------------------------------------------------------------------

fn find_control_plane() -> Result<PathBuf, String> {
    // Env override first
    if let Ok(p) = env::var("S2_CONTROL_PLANE_PATH") {
        let path = PathBuf::from(&p);
        if path.exists() {
            return Ok(path);
        }
        return Err(format!("S2_CONTROL_PLANE_PATH={p} does not exist"));
    }

    // Walk up from CWD looking for Systems/Runtimes/control-plane.json
    let mut current = env::current_dir().map_err(|e| e.to_string())?;
    loop {
        let candidate = current.join("Systems/Runtimes/control-plane.json");
        if candidate.exists() {
            return Ok(candidate);
        }
        // Also try direct path (we might already be inside Systems/Runtimes)
        let candidate2 = current.join("control-plane.json");
        if candidate2.exists() && current.ends_with("Runtimes") {
            return Ok(candidate2);
        }
        if !current.pop() {
            break;
        }
    }

    // Hard fallback to the canonical location
    let fallback = Path::new(
        "/Users/zachshallbetter/Projects/S2Forge/Systems/Runtimes/control-plane.json",
    )
    .to_path_buf();
    if fallback.exists() {
        return Ok(fallback);
    }

    Err("control-plane.json not found — set S2_CONTROL_PLANE_PATH to override".to_string())
}

fn load_control_plane() -> Result<ControlPlane, Box<dyn std::error::Error>> {
    let path = find_control_plane().map_err(|e| format!("fleet registry: {e}"))?;
    let raw = std::fs::read_to_string(&path)?;
    let cp: ControlPlane = serde_json::from_str(&raw)?;
    Ok(cp)
}

// ---------------------------------------------------------------------------
// Health check helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum LocalHealth {
    Up,
    Down,
    Unknown,
}

impl LocalHealth {
    fn display(&self) -> ColoredString {
        match self {
            LocalHealth::Up => "✓ up".green(),
            LocalHealth::Down => "✗ down".red(),
            LocalHealth::Unknown => "? no-response".yellow(),
        }
    }
}

async fn ping_local(port: u16) -> LocalHealth {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
    {
        Ok(c) => c,
        Err(_) => return LocalHealth::Unknown,
    };
    let url = format!("http://localhost:{port}/health");
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => LocalHealth::Up,
        Ok(_) => LocalHealth::Down,
        Err(e) if e.is_timeout() || e.is_connect() => LocalHealth::Unknown,
        Err(_) => LocalHealth::Down,
    }
}

// ---------------------------------------------------------------------------
// Railway GraphQL helper
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct GqlRequest {
    query: String,
    variables: Value,
}

#[derive(Debug, Deserialize)]
struct GqlResponse {
    data: Option<Value>,
    #[serde(default)]
    errors: Vec<Value>,
}

async fn railway_service_status(token: &str, name: &str) -> Option<String> {
    let query = r#"
        query ServiceStatus($name: String!) {
          services(filter: { name: $name }) {
            edges {
              node {
                name
                deployments(first: 1) {
                  edges {
                    node {
                      status
                      createdAt
                    }
                  }
                }
              }
            }
          }
        }
    "#
    .to_string();

    let body = GqlRequest {
        query,
        variables: serde_json::json!({ "name": name }),
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;

    let resp = client
        .post("https://backboard.railway.app/graphql/v2")
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .ok()?;

    let gql: GqlResponse = resp.json().await.ok()?;

    if !gql.errors.is_empty() {
        return Some("gql-error".to_string());
    }

    let status = gql
        .data?
        .get("services")?
        .get("edges")?
        .as_array()?
        .first()?
        .get("node")?
        .get("deployments")?
        .get("edges")?
        .as_array()?
        .first()?
        .get("node")?
        .get("status")?
        .as_str()
        .map(str::to_string)?;

    Some(status)
}

fn format_railway_status(s: &str) -> ColoredString {
    match s.to_uppercase().as_str() {
        "SUCCESS" => s.green(),
        "FAILED" | "CRASHED" => s.red().bold(),
        "DEPLOYING" | "BUILDING" => s.yellow(),
        _ => s.normal(),
    }
}

// ---------------------------------------------------------------------------
// Column printing helpers
// ---------------------------------------------------------------------------

const W_NAME: usize = 38;
const W_PORT: usize = 6;
const W_LANG: usize = 12;
const W_RAIL: usize = 16;
const W_LOCAL: usize = 14;

fn print_header() {
    println!(
        "{:<W_NAME$} {:>W_PORT$}  {:<W_LANG$} {:<W_RAIL$} {:<W_LOCAL$}",
        "name".bold(),
        "port".bold(),
        "lang".bold(),
        "railway".bold(),
        "local".bold(),
    );
    println!("{}", "─".repeat(W_NAME + W_PORT + W_LANG + W_RAIL + W_LOCAL + 6));
}

fn print_row(
    name: &str,
    port: u16,
    lang: &str,
    railway: Option<&str>,
    local: &LocalHealth,
) {
    let rail_col = match railway {
        Some(s) => format_railway_status(s).to_string(),
        None => "—".dimmed().to_string(),
    };
    println!(
        "{:<W_NAME$} {:>W_PORT$}  {:<W_LANG$} {:<W_RAIL$} {}",
        name,
        port,
        lang,
        rail_col,
        local.display(),
    );
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

async fn cmd_status() -> Result<(), Box<dyn std::error::Error>> {
    let cp = load_control_plane()?;
    let token = env::var("RAILWAY_TOKEN").ok();

    println!("{}", "\n── BROKERS ────────────────────────────────────────────────────".bold());
    print_header();
    check_brokers(&cp.brokers, token.as_deref()).await;

    println!("{}", "\n── LIVE RUNTIMES ──────────────────────────────────────────────".bold());
    print_header();
    check_runtimes(&cp.live_runtimes, token.as_deref()).await;

    if !cp.scaffolded_runtimes.is_empty() {
        println!("{}", "\n── SCAFFOLDED RUNTIMES ────────────────────────────────────────".bold());
        print_header();
        check_runtimes(&cp.scaffolded_runtimes, token.as_deref()).await;
    }

    Ok(())
}

async fn cmd_brokers() -> Result<(), Box<dyn std::error::Error>> {
    let cp = load_control_plane()?;
    let token = env::var("RAILWAY_TOKEN").ok();

    println!("{}", "\n── BROKERS ────────────────────────────────────────────────────".bold());
    print_header();

    let futures: Vec<_> = cp
        .brokers
        .into_iter()
        .map(|b| {
            let token = token.clone();
            async move {
                let local = ping_local(b.port).await;
                let rail = if let Some(t) = &token {
                    railway_service_status(t, &b.name).await
                } else {
                    None
                };
                (b.name, b.port, b.language, rail, local, b.note, b.status, b.gaps)
            }
        })
        .collect();

    let results = join_all(futures).await;
    for (name, port, lang, rail, local, note, broker_status, gaps) in &results {
        let display_name = if let Some(s) = broker_status {
            format!("{name} [{s}]")
        } else {
            name.clone()
        };
        print_row(&display_name, *port, lang, rail.as_deref(), local);
        if let Some(n) = note {
            println!("  {} {}", "note:".dimmed(), n.dimmed());
        }
        if !gaps.is_empty() {
            for g in gaps {
                println!("  {} {}", "gap:".yellow(), g.yellow());
            }
        }
    }
    Ok(())
}

async fn check_brokers(brokers: &[Broker], token: Option<&str>) {
    let futures: Vec<_> = brokers
        .iter()
        .map(|b| async move {
            let local = ping_local(b.port).await;
            let rail = if let Some(t) = token {
                railway_service_status(t, &b.name).await
            } else {
                None
            };
            (b.name.as_str(), b.port, b.language.as_str(), rail, local)
        })
        .collect();

    let results = join_all(futures).await;
    for (name, port, lang, rail, local) in &results {
        print_row(name, *port, lang, rail.as_deref(), local);
    }
}

async fn check_runtimes(runtimes: &[Runtime], token: Option<&str>) {
    let futures: Vec<_> = runtimes
        .iter()
        .map(|r| async move {
            let local = ping_local(r.port).await;
            let short_name = r.name.split('/').last().unwrap_or(&r.name);
            let rail = if let Some(t) = token {
                railway_service_status(t, short_name).await
            } else {
                None
            };
            (r.name.as_str(), r.port, r.language.as_str(), rail, local, &r.gaps)
        })
        .collect();

    let results = join_all(futures).await;
    for (name, port, lang, rail, local, gaps) in &results {
        print_row(name, *port, lang, rail.as_deref(), local);
        if !gaps.is_empty() {
            for g in *gaps {
                println!("  {} {}", "gap:".yellow(), g.yellow());
            }
        }
    }
}

async fn cmd_runtimes() -> Result<(), Box<dyn std::error::Error>> {
    let cp = load_control_plane()?;
    let token = env::var("RAILWAY_TOKEN").ok();

    println!("{}", "\n── LIVE RUNTIMES ──────────────────────────────────────────────".bold());
    print_header();
    check_runtimes(&cp.live_runtimes, token.as_deref()).await;

    if !cp.scaffolded_runtimes.is_empty() {
        println!("{}", "\n── SCAFFOLDED RUNTIMES ────────────────────────────────────────".bold());
        print_header();
        check_runtimes(&cp.scaffolded_runtimes, token.as_deref()).await;
    }

    Ok(())
}

async fn cmd_health(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cp = load_control_plane()?;

    // Find in brokers
    let broker_match = cp.brokers.iter().find(|b| b.name == name);
    let runtime_match = cp
        .live_runtimes
        .iter()
        .chain(cp.scaffolded_runtimes.iter())
        .find(|r| r.name == name || r.name.split('/').last() == Some(name));

    let (port, lang) = if let Some(b) = broker_match {
        (b.port, b.language.as_str())
    } else if let Some(r) = runtime_match {
        (r.port, r.language.as_str())
    } else {
        eprintln!("{} service '{}' not found in control-plane.json", "error:".red().bold(), name);
        std::process::exit(1);
    };

    println!(
        "{} {} | port {} | lang {}",
        "health:".bold(),
        name,
        port,
        lang
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let url = format!("http://localhost:{port}/health");

    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            // Attempt pretty JSON
            let pretty = serde_json::from_str::<Value>(&body)
                .map(|v| serde_json::to_string_pretty(&v).unwrap_or(body.clone()))
                .unwrap_or(body);
            if status.is_success() {
                println!("  {} {}", "status:".green(), status);
            } else {
                println!("  {} {}", "status:".red(), status);
            }
            println!("{}", pretty);
        }
        Err(e) if e.is_timeout() => {
            println!("  {} timed out", "status:".yellow());
        }
        Err(e) if e.is_connect() => {
            println!("  {} connection refused — service not running locally", "status:".red());
        }
        Err(e) => {
            println!("  {} {}", "error:".red(), e);
        }
    }

    Ok(())
}

async fn cmd_gaps() -> Result<(), Box<dyn std::error::Error>> {
    let cp = load_control_plane()?;
    let mut found = false;

    println!("{}", "\n── GAPS ───────────────────────────────────────────────────────".bold());

    for b in &cp.brokers {
        for g in &b.gaps {
            println!("  {:<38}  {}", b.name.yellow(), g);
            found = true;
        }
    }

    for r in cp.live_runtimes.iter().chain(cp.scaffolded_runtimes.iter()) {
        for g in &r.gaps {
            println!("  {:<38}  {}", r.name.yellow(), g);
            found = true;
        }
    }

    if !found {
        println!("  {}", "no gaps recorded".dimmed());
    }

    Ok(())
}

async fn cmd_watch(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{} {}",
        "starting webhook listener on port".green(),
        port
    );
    println!(
        "point Railway webhooks at your tunnel → http://localhost:{}/webhook",
        port
    );

    let app = Router::new().route("/webhook", post(webhook_handler));
    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn webhook_handler(Json(payload): Json<Value>) {
    println!("\n{}", "=== incoming webhook ===".blue().bold());
    if let Some(status) = payload.get("status") {
        println!("status: {}", status.as_str().unwrap_or("unknown").yellow());
    }
    let pretty = serde_json::to_string_pretty(&payload).unwrap_or_default();
    println!("{}", pretty);
    println!("{}", "========================".blue().bold());
}
