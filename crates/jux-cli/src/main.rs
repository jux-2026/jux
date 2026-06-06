use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use jux_core::{RunLoop, RunLoopOutput, SqliteWorkspaceStore, Step, StepPayload};
use rig::{client::CompletionClient, completion::Prompt, providers::deepseek};
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

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LlmProviderName {
    Deepseek,
}

fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();

    match cli.command {
        Some(Command::Run(args)) => handle_run(args, cli.output),
        None => Ok(()),
    }
}

fn handle_run(args: RunArgs, output_format: OutputFormat) -> Result<()> {
    tracing::info!(
        provider = ?args.provider,
        workspace = %args.workspace.display(),
        "starting run command"
    );

    let output = match args.provider {
        LlmProviderName::Deepseek => {
            let prompt = deepseek_prompt(args.deepseek_base_url, args.deepseek_model)?;
            run_with_prompt(args.workspace, prompt, args.request)?
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

fn run_with_prompt<P>(
    workspace: PathBuf,
    prompt: P,
    request: String,
) -> Result<jux_core::RunLoopOutput>
where
    P: Prompt,
{
    let runtime = Builder::new_current_thread().enable_all().build()?;
    let store = SqliteWorkspaceStore::new(workspace);
    let run_loop = RunLoop::new(store, prompt);
    Ok(runtime.block_on(run_loop.run(request))?)
}

fn deepseek_prompt(deepseek_base_url: String, deepseek_model: String) -> Result<impl Prompt> {
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

    Ok(client.agent(deepseek_model).build())
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .try_init();
}
