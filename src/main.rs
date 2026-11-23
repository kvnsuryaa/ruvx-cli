use anyhow::{Context, Result, ensure};
use clap::Parser;
use ruvx::{config::Paths, model::Mode, session::Session};
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
};

#[derive(Parser)]
#[command(
    version,
    about = "A small terminal coding agent. Plan first; Build with Ask or explicit BYPASS approval policy."
)]
struct Cli {
    /// Existing workspace directory (an empty directory is fine for a new project).
    #[arg(conflicts_with = "resume")]
    path: Option<PathBuf>,
    /// Resume a saved session; no requests or tools are replayed automatically.
    #[arg(long, value_name = "SESSION_ID")]
    resume: Option<String>,
    /// Start in Build and skip shell/MCP approvals for this process.
    #[arg(long)]
    bypass: bool,
}

#[tokio::main(worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    match start().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ruvx: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
async fn start() -> Result<()> {
    let cli = Cli::parse();
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "Interactive terminal required. Headless/pipe mode is not supported in v1; use --help for usage."
    );
    let paths = Paths::discover()?;
    let mut config = paths.load()?;
    config.bypass = cli.bypass;
    let mut session = if let Some(id) = cli.resume {
        Session::resume(&paths, &id)?
    } else {
        Session::create(
            &paths,
            &cli.path
                .unwrap_or(std::env::current_dir().context("Cannot read working directory")?),
        )?
    };
    if cli.bypass {
        session.set_mode(Mode::Build)?;
        session.record(
            "approval.policy_changed",
            serde_json::json!({"policy":"bypass","source":"cli"}),
        )?;
    }
    ruvx::ui::run(paths, config, session).await
}
