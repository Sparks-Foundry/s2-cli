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
    #[serde(default)]
    product_services: Vec<ProductService>,
}

#[derive(Debug, Deserialize, Clone)]
struct ProductService {
    name: String,
    port: u16,
    product: String,
    #[serde(default = "default_health_path")]
    health_path: String,
    /// Full path to probe for auth-gate (e.g. "/coach/oauth/google/start"). None = skip.
    #[serde(default)]
    probe: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    auth: Option<String>,
}

fn default_health_path() -> String {
    "/health".to_string()
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
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    auth: Option<String>,
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
    /// Run behavioral smoke tests against live services to catch regressions
    Verify {
        /// Name or substring to filter services (e.g. "snack", "text-runtime"). Omit for all live services.
        filter: Option<String>,
    },
    /// Manage git worktrees for category repos (clean-main-is-sacred — see /WORKFLOW.md)
    Worktree {
        #[command(subcommand)]
        action: WorktreeAction,
    },
}

#[derive(Subcommand)]
enum WorktreeAction {
    /// Add a worktree for <category> on a new branch feat/<name> (or detached at --at <sha>)
    Add {
        /// Category repo name (e.g. "generation", "compute", "state")
        category: String,
        /// Short work name; becomes branch feat/<name> and dir .wt/<category>--<name>
        name: String,
        /// Detach at a commit/SHA instead of creating a branch (hotfix on the running commit)
        #[arg(long)]
        at: Option<String>,
    },
    /// List worktrees across all category repos
    Ls,
    /// Remove a worktree and prune
    Rm {
        category: String,
        name: String,
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
        Commands::Verify { filter } => {
            let any_failed = cmd_verify(filter.as_deref()).await?;
            if any_failed {
                std::process::exit(1);
            }
        }
        Commands::Worktree { action } => cmd_worktree(action)?,
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Worktree command — clean-main-is-sacred (see /WORKFLOW.md)
// ---------------------------------------------------------------------------

/// Walk up from CWD to the fleet root (the dir holding both Systems/ and Products/).
fn find_fleet_root() -> Result<PathBuf, String> {
    if let Ok(p) = env::var("S2_FLEET_ROOT") {
        let path = PathBuf::from(&p);
        if path.join("Systems").is_dir() {
            return Ok(path);
        }
    }
    let mut current = env::current_dir().map_err(|e| e.to_string())?;
    loop {
        if current.join("Systems/Runtimes").is_dir() && current.join("Products").is_dir() {
            return Ok(current);
        }
        if !current.pop() {
            break;
        }
    }
    Err("fleet root not found (no ancestor with Systems/Runtimes + Products) — set S2_FLEET_ROOT".into())
}

/// Resolve a category name to its repo dir. Tries Systems/Runtimes/<cat> then Systems/<cat>.
fn resolve_category_repo(root: &Path, category: &str) -> Result<PathBuf, String> {
    for c in [
        root.join("Systems/Runtimes").join(category),
        root.join("Systems").join(category),
    ] {
        if c.join(".git").exists() {
            return Ok(c);
        }
    }
    Err(format!(
        "category repo '{category}' not found under Systems/Runtimes/ or Systems/ (no .git there)"
    ))
}

fn run_git(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let status = std::process::Command::new("git").args(args).status()?;
    if !status.success() {
        return Err(format!("git {} exited with failure", args.join(" ")).into());
    }
    Ok(())
}

fn cmd_worktree(action: WorktreeAction) -> Result<(), Box<dyn std::error::Error>> {
    let root = find_fleet_root().map_err(|e| format!("worktree: {e}"))?;
    match action {
        WorktreeAction::Add { category, name, at } => {
            worktree_add(&root, &category, &name, at.as_deref())
        }
        WorktreeAction::Ls => worktree_ls(&root),
        WorktreeAction::Rm { category, name } => worktree_rm(&root, &category, &name),
    }
}

fn worktree_add(
    root: &Path,
    category: &str,
    name: &str,
    at: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo = resolve_category_repo(root, category)?;
    let parent = repo
        .parent()
        .ok_or("category repo has no parent directory")?;
    let wt_dir = parent.join(".wt").join(format!("{category}--{name}"));
    if wt_dir.exists() {
        return Err(format!("worktree already exists: {}", wt_dir.display()).into());
    }
    std::fs::create_dir_all(parent.join(".wt"))?;

    let repo_s = repo.to_string_lossy().to_string();
    let wt_s = wt_dir.to_string_lossy().to_string();
    let mut args = vec![
        "-C".to_string(),
        repo_s,
        "worktree".to_string(),
        "add".to_string(),
    ];
    match at {
        Some(sha) => {
            args.push("--detach".to_string());
            args.push(wt_s);
            args.push(sha.to_string());
        }
        None => {
            args.push("-b".to_string());
            args.push(format!("feat/{name}"));
            args.push(wt_s);
        }
    }
    run_git(&args)?;

    // Shared per-category build cache (avoids a cold Rust target/ per worktree).
    let cache = parent.join(".wt").join(".cargo-target").join(category);
    let _ = std::fs::create_dir_all(&cache);

    println!("{} {}", "worktree:".green().bold(), wt_dir.display());
    match at {
        Some(sha) => println!("  {} detached at {sha}", "branch:".dimmed()),
        None => println!("  {} feat/{name}", "branch:".dimmed()),
    }
    println!("  {} cd {}", "next:".dimmed(), wt_dir.display());
    println!(
        "       {} {}",
        "export CARGO_TARGET_DIR=".dimmed(),
        cache.display()
    );
    println!(
        "       {}",
        "railway link --project <id> --service <svc> --environment production".dimmed()
    );
    Ok(())
}

fn worktree_ls(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", "\n── WORKTREES ──────────────────────────────────────────────────".bold());
    let mut search_dirs = vec![root.join("Systems/Runtimes"), root.join("Systems")];
    search_dirs.retain(|d| d.is_dir());

    let mut found_any = false;
    for base in &search_dirs {
        let entries = match std::fs::read_dir(base) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let repo = entry.path();
            if !repo.join(".git").exists() {
                continue;
            }
            let out = std::process::Command::new("git")
                .args(["-C", &repo.to_string_lossy(), "worktree", "list", "--porcelain"])
                .output();
            let Ok(out) = out else { continue };
            let text = String::from_utf8_lossy(&out.stdout);
            // Each worktree block starts with "worktree <path>"; the first is the main checkout.
            let mut paths: Vec<&str> = text
                .lines()
                .filter_map(|l| l.strip_prefix("worktree "))
                .collect();
            if paths.len() <= 1 {
                continue; // only the main checkout, no linked worktrees
            }
            let cat = repo.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            // Skip the first (main checkout); print the rest.
            paths.remove(0);
            for p in paths {
                println!("  {:<14} {}", cat.yellow(), p);
                found_any = true;
            }
        }
    }
    if !found_any {
        println!("  {}", "no linked worktrees — every main checkout is clean".dimmed());
    }
    Ok(())
}

fn worktree_rm(root: &Path, category: &str, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let repo = resolve_category_repo(root, category)?;
    let parent = repo.parent().ok_or("category repo has no parent directory")?;
    let wt_dir = parent.join(".wt").join(format!("{category}--{name}"));
    let repo_s = repo.to_string_lossy().to_string();
    run_git(&[
        "-C".to_string(),
        repo_s.clone(),
        "worktree".to_string(),
        "remove".to_string(),
        wt_dir.to_string_lossy().to_string(),
    ])?;
    run_git(&[
        "-C".to_string(),
        repo_s.clone(),
        "worktree".to_string(),
        "prune".to_string(),
    ])?;
    println!("{} removed {}", "worktree:".green().bold(), wt_dir.display());

    // Safe-delete the feat/<name> branch the worktree was created on. `-d` refuses to
    // delete unmerged branches, so this never loses work — it just clears merged cruft.
    let branch = format!("feat/{name}");
    let out = std::process::Command::new("git")
        .args(["-C", &repo_s, "branch", "-d", &branch])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            println!("  {} {branch}", "branch deleted:".dimmed());
        }
        Ok(_) => {
            // Unmerged or detached (no such branch) — leave it and tell the operator.
            println!(
                "  {} {branch} kept (unmerged or detached) — `git -C {} branch -D {branch}` to force",
                "note:".yellow(),
                repo_s
            );
        }
        Err(_) => {}
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

// ---------------------------------------------------------------------------
// Verify command — behavioral smoke tests
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum CheckResult {
    Pass(String),
    Fail(String),
    Skip(String),
}

impl CheckResult {
    fn display(&self) -> ColoredString {
        match self {
            CheckResult::Pass(msg) => format!("✓  {msg}").green(),
            CheckResult::Fail(msg) => format!("✗  {msg}").red().bold(),
            CheckResult::Skip(msg) => format!("–  {msg}").dimmed(),
        }
    }

    fn is_fail(&self) -> bool {
        matches!(self, CheckResult::Fail(_))
    }
}

async fn verify_liveness(client: &reqwest::Client, port: u16, health_path: &str) -> CheckResult {
    let url = format!("http://localhost:{port}{health_path}");
    match client.get(&url).send().await {
        Ok(r) if r.status().is_success() => CheckResult::Pass(r.status().to_string()),
        Ok(r) => CheckResult::Fail(format!("unexpected status {}", r.status())),
        Err(e) if e.is_connect() => CheckResult::Fail("connection refused".to_string()),
        Err(e) if e.is_timeout() => CheckResult::Fail("timeout".to_string()),
        Err(e) => CheckResult::Fail(e.to_string()),
    }
}

// Probe an authenticated endpoint without a token — expect 401 or 403.
// A 200 means auth is being bypassed; a 5xx means middleware is crashing.
async fn verify_auth_gate(client: &reqwest::Client, port: u16, probe_path: &str) -> CheckResult {
    let url = format!("http://localhost:{port}{probe_path}");
    match client.post(&url).send().await {
        Ok(r) => {
            let s = r.status().as_u16();
            if s == 401 || s == 403 {
                CheckResult::Pass(format!("{s} (auth rejected as expected)"))
            } else if s == 404 {
                // Endpoint doesn't exist at that path — not a security issue, just wrong path.
                CheckResult::Skip(format!("404 on {probe_path} — endpoint path may differ"))
            } else if s >= 500 {
                CheckResult::Fail(format!(
                    "{s} — middleware may be panicking on unauthenticated requests"
                ))
            } else {
                CheckResult::Fail(format!(
                    "{s} — expected 401/403; auth may be bypassed or disabled"
                ))
            }
        }
        Err(e) if e.is_connect() => CheckResult::Skip("service not running".to_string()),
        Err(e) if e.is_timeout() => CheckResult::Fail("timeout".to_string()),
        Err(e) => CheckResult::Fail(e.to_string()),
    }
}

async fn verify_manifest(client: &reqwest::Client, port: u16) -> CheckResult {
    let url = format!("http://localhost:{port}/v1/control/manifest");
    match client.get(&url).send().await {
        Ok(r) if r.status().is_success() => {
            let body = r.text().await.unwrap_or_default();
            if serde_json::from_str::<Value>(&body).is_ok() {
                CheckResult::Pass("200 OK, valid JSON".to_string())
            } else {
                CheckResult::Fail("200 OK but response is not valid JSON".to_string())
            }
        }
        Ok(r) if r.status() == 404 => CheckResult::Skip("endpoint not exposed".to_string()),
        Ok(r) if r.status() == 401 || r.status() == 403 => {
            CheckResult::Skip("requires auth (not probing further)".to_string())
        }
        Ok(r) => CheckResult::Fail(format!("unexpected status {}", r.status())),
        Err(e) if e.is_connect() => CheckResult::Skip("service not running".to_string()),
        Err(e) if e.is_timeout() => CheckResult::Fail("timeout".to_string()),
        Err(e) => CheckResult::Fail(e.to_string()),
    }
}

struct ServiceCheck<'a> {
    name: &'a str,
    port: u16,
    /// Full path for liveness probe, e.g. "/health" or "/v1/fleet/status".
    health_path: &'a str,
    /// Full path for auth-gate probe, e.g. "/v1/generate_text" or "/coach/oauth/google/start".
    /// None = skip auth-gate for this service.
    probe_path: Option<String>,
}

async fn run_checks(svc: ServiceCheck<'_>, client: &reqwest::Client) -> (String, bool) {
    let liveness = verify_liveness(client, svc.port, svc.health_path).await;

    let auth = if let Some(ref path) = svc.probe_path {
        verify_auth_gate(client, svc.port, path).await
    } else {
        CheckResult::Skip("no auth probe declared".to_string())
    };

    let manifest = verify_manifest(client, svc.port).await;

    let any_fail = liveness.is_fail() || auth.is_fail() || manifest.is_fail();

    let header = if any_fail {
        format!("── {} ─", svc.name).red().bold().to_string()
    } else {
        format!("── {} ─", svc.name).bold().to_string()
    };

    let block = format!(
        "{header}\n  {:<12} {}\n  {:<12} {}\n  {:<12} {}",
        "liveness",
        liveness.display(),
        "auth-gate",
        auth.display(),
        "manifest",
        manifest.display(),
    );

    (block, any_fail)
}

async fn cmd_verify(filter: Option<&str>) -> Result<bool, Box<dyn std::error::Error>> {
    let cp = load_control_plane()?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;

    let f_lower = filter.map(|f| f.to_lowercase());
    let matches = |name: &str, product: Option<&str>| -> bool {
        match &f_lower {
            None => true,
            Some(f) => {
                name.to_lowercase().contains(f.as_str())
                    || product.map(|p| p.to_lowercase().contains(f.as_str())).unwrap_or(false)
            }
        }
    };

    let runtimes: Vec<&Runtime> = cp
        .live_runtimes
        .iter()
        .filter(|r| matches(&r.name, None))
        .collect();

    let product_svcs: Vec<&ProductService> = cp
        .product_services
        .iter()
        .filter(|p| matches(&p.name, Some(&p.product)))
        .collect();

    if runtimes.is_empty() && product_svcs.is_empty() && filter.is_some() {
        eprintln!(
            "{} no services match '{}'",
            "error:".red().bold(),
            filter.unwrap()
        );
        return Ok(true);
    }

    let label = match filter {
        Some(f) => format!("VERIFY: {f}"),
        None => "VERIFY: all live runtimes".to_string(),
    };
    println!("\n{}", format!("── {label} ──────────────────────────────────────────────").bold());

    let runtime_futures: Vec<_> = runtimes
        .iter()
        .map(|r| {
            let client = &client;
            let skip_auth = r
                .auth
                .as_deref()
                .map(|a| a.contains("internal-only"))
                .unwrap_or(false);
            let probe_path = if skip_auth {
                None
            } else {
                r.tools.first().map(|t| format!("/v1/{t}"))
            };
            let svc = ServiceCheck {
                name: &r.name,
                port: r.port,
                health_path: "/health",
                probe_path,
            };
            run_checks(svc, client)
        })
        .collect();

    let product_futures: Vec<_> = product_svcs
        .iter()
        .map(|p| {
            let client = &client;
            let svc = ServiceCheck {
                name: &p.name,
                port: p.port,
                health_path: &p.health_path,
                probe_path: p.probe.clone(),
            };
            run_checks(svc, client)
        })
        .collect();

    let (runtime_results, product_results) =
        tokio::join!(join_all(runtime_futures), join_all(product_futures));
    let results: Vec<_> = runtime_results.into_iter().chain(product_results).collect();

    let mut any_failed = false;
    for (block, failed) in &results {
        println!("{block}");
        if *failed {
            any_failed = true;
        }
    }

    println!();
    let total = results.len();
    let failed_count = results.iter().filter(|(_, f)| *f).count();

    if failed_count == 0 {
        println!(
            "{}",
            format!("  all {total} services passed").green().bold()
        );
    } else {
        println!(
            "{}",
            format!("  {failed_count}/{total} services have failing checks").red().bold()
        );
    }

    Ok(any_failed)
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
