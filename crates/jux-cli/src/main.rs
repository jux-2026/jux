use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use jux_core::{
    Run, RunLoop, RunLoopOutput, Session, SessionId, SqliteWorkspaceStore, Step, StepPayload,
    Workspace,
};
use rig::{client::CompletionClient, completion::CompletionModel, providers::deepseek};
use serde::Serialize;
use std::env;
use std::path::PathBuf;
use tokio::runtime::Builder;
use tracing_subscriber::EnvFilter;

const DEFAULT_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";
const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek-chat";

#[derive(Debug, Parser)]
#[command(
    name = "jux",
    version = jux_core::version(),
    about = "Jux agent command line interface."
)]
struct Cli {
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text, help = "Command output format.")]
    output: OutputFormat,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Run a request through the local Jux agent loop.")]
    Run(RunArgs),
    #[command(about = "Inspect local session state.")]
    Session(SessionCommand),
}

#[derive(Debug, Parser)]
struct SessionCommand {
    #[command(subcommand)]
    command: SessionSubcommand,
}

#[derive(Debug, Subcommand)]
enum SessionSubcommand {
    #[command(about = "Show a session and its recorded runs and steps.")]
    Show(SessionShowArgs),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
    Yaml,
}

#[derive(Debug, Parser)]
struct RunArgs {
    #[arg(help = "User request that starts the run.")]
    request: String,

    #[arg(long, default_value = ".", help = "Workspace root directory.")]
    workspace: PathBuf,

    #[arg(long, value_enum, default_value_t = LlmProviderName::Deepseek, help = "LLM provider.")]
    provider: LlmProviderName,

    #[arg(long, env = "JUX_DEEPSEEK_BASE_URL", default_value = DEFAULT_DEEPSEEK_BASE_URL, help = "DeepSeek API base URL.")]
    deepseek_base_url: String,

    #[arg(long, env = "JUX_DEEPSEEK_MODEL", default_value = DEFAULT_DEEPSEEK_MODEL, help = "DeepSeek model name.")]
    deepseek_model: String,
}

#[derive(Debug, Parser)]
struct SessionShowArgs {
    #[arg(help = "Session id. Defaults to the active session in the workspace.")]
    session_id: Option<String>,

    #[arg(long, default_value = ".", help = "Workspace root directory.")]
    workspace: PathBuf,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LlmProviderName {
    Deepseek,
}

fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();

    match cli.command {
        Some(Command::Run(args)) => handle_run(args, cli.output),
        Some(Command::Session(command)) => handle_session(command, cli.output),
        None => Ok(()),
    }
}

fn handle_session(command: SessionCommand, output_format: OutputFormat) -> Result<()> {
    match command.command {
        SessionSubcommand::Show(args) => handle_session_show(args, output_format),
    }
}

fn handle_session_show(args: SessionShowArgs, output_format: OutputFormat) -> Result<()> {
    let store = SqliteWorkspaceStore::new(args.workspace);
    let workspace = store.load_workspace()?;
    let session = match args.session_id {
        Some(session_id) => store.load_session(&SessionId::from(session_id))?,
        None => store.load_active_session()?,
    };
    let runs = store.load_session_runs(&session.id)?;
    let steps = store.load_session_steps(&session.id)?;

    print_session_show_output(
        &SessionShowOutput::new(workspace, session, runs, steps),
        output_format,
    )
}

fn handle_run(args: RunArgs, output_format: OutputFormat) -> Result<()> {
    tracing::info!(
        provider = ?args.provider,
        workspace = %args.workspace.display(),
        "starting run command"
    );

    let output = match args.provider {
        LlmProviderName::Deepseek => {
            let model = deepseek_model(args.deepseek_base_url, args.deepseek_model)?;
            run_with_model(args.workspace, model, args.request)?
        }
    };
    print_run_output(&output, output_format)?;

    tracing::info!(
        run_id = %output.run.id,
        status = ?output.run.status,
        step_count = output.steps.len(),
        "run command completed"
    );

    Ok(())
}

fn print_run_output(output: &RunLoopOutput, output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Text => {
            if let Some(answer) = &output.answer {
                println!("{answer}");
            }
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&RunCommandOutput::from(output))?
            );
        }
        OutputFormat::Yaml => {
            print!(
                "{}",
                serde_yaml::to_string(&RunCommandOutput::from(output))?
            );
        }
    }

    Ok(())
}

fn print_session_show_output(
    output: &SessionShowOutput,
    output_format: OutputFormat,
) -> Result<()> {
    match output_format {
        OutputFormat::Text => {
            println!("Session {}", output.session.id);
            println!();
            println!("Workspace:");
            println!("  id: {}", output.workspace.id);
            println!("  root: {}", output.workspace.root.display());
            println!(
                "  active_session_id: {}",
                output.workspace.active_session_id
            );
            println!();
            println!("Runs:");
            for run in &output.runs {
                println!("  - {} {} {}", run.id, run.status, run.request);
                for step in &run.steps {
                    println!("    - {} {}", step.id, step.kind);
                }
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(output)?);
        }
        OutputFormat::Yaml => {
            print!("{}", serde_yaml::to_string(output)?);
        }
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct SessionShowOutput {
    workspace: Workspace,
    session: Session,
    runs: Vec<SessionRunOutput>,
}

impl SessionShowOutput {
    fn new(workspace: Workspace, session: Session, runs: Vec<Run>, steps: Vec<Step>) -> Self {
        let runs = runs
            .into_iter()
            .map(|run| {
                let run_steps = steps
                    .iter()
                    .filter(|step| step.id.run_id() == run.id)
                    .map(SessionStepOutput::from)
                    .collect();
                SessionRunOutput::new(run, run_steps)
            })
            .collect();

        Self {
            workspace,
            session,
            runs,
        }
    }
}

#[derive(Debug, Serialize)]
struct SessionRunOutput {
    id: String,
    request: String,
    status: String,
    created_at: u128,
    updated_at: u128,
    steps: Vec<SessionStepOutput>,
}

impl SessionRunOutput {
    fn new(run: Run, steps: Vec<SessionStepOutput>) -> Self {
        Self {
            id: run.id.to_string(),
            request: run.request,
            status: format!("{:?}", run.status),
            created_at: run.created_at,
            updated_at: run.updated_at,
            steps,
        }
    }
}

#[derive(Debug, Serialize)]
struct SessionStepOutput {
    id: String,
    kind: String,
    payload: StepPayload,
    created_at: u128,
    updated_at: u128,
}

impl From<&Step> for SessionStepOutput {
    fn from(step: &Step) -> Self {
        Self {
            id: step.id.to_string(),
            kind: format!("{:?}", step.kind),
            payload: step.payload.clone(),
            created_at: step.created_at,
            updated_at: step.updated_at,
        }
    }
}

#[derive(Debug, Serialize)]
struct RunCommandOutput {
    workspace_id: String,
    session_id: String,
    run_id: String,
    request: String,
    status: String,
    answer: Option<String>,
    created_at: u128,
    updated_at: u128,
    steps: Vec<RunStepOutput>,
}

impl From<&RunLoopOutput> for RunCommandOutput {
    fn from(output: &RunLoopOutput) -> Self {
        Self {
            workspace_id: output.run.id.workspace_id().to_string(),
            session_id: output.run.id.session_id().to_string(),
            run_id: output.run.id.to_string(),
            request: output.run.request.clone(),
            status: format!("{:?}", output.run.status),
            answer: output.answer.clone(),
            created_at: output.run.created_at,
            updated_at: output.run.updated_at,
            steps: output.steps.iter().map(RunStepOutput::from).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct RunStepOutput {
    id: String,
    kind: String,
    payload: StepPayload,
    created_at: u128,
    updated_at: u128,
}

impl From<&Step> for RunStepOutput {
    fn from(step: &Step) -> Self {
        Self {
            id: step.id.to_string(),
            kind: format!("{:?}", step.kind),
            payload: step.payload.clone(),
            created_at: step.created_at,
            updated_at: step.updated_at,
        }
    }
}

fn run_with_model<M>(
    workspace: PathBuf,
    model: M,
    request: String,
) -> Result<jux_core::RunLoopOutput>
where
    M: CompletionModel,
{
    let runtime = Builder::new_current_thread().enable_all().build()?;
    let store = SqliteWorkspaceStore::new(workspace);
    let run_loop = RunLoop::new(store, model);
    Ok(runtime.block_on(run_loop.run(request))?)
}

fn deepseek_model(
    deepseek_base_url: String,
    deepseek_model: String,
) -> Result<impl CompletionModel> {
    let api_key = env::var("JUX_DEEPSEEK_API_KEY")
        .context("JUX_DEEPSEEK_API_KEY must be set when using the deepseek provider")?;
    tracing::debug!(
        base_url = %deepseek_base_url,
        model = %deepseek_model,
        "building deepseek prompt"
    );
    let client = deepseek::Client::builder()
        .base_url(deepseek_base_url.trim_end_matches('/'))
        .api_key(&api_key)
        .build()?;

    Ok(client.completion_model(deepseek_model))
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .try_init();
}
