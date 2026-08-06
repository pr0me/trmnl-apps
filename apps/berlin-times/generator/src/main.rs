use std::path::PathBuf;

use anyhow::Context;
use berlin_times::GenerateOptions;
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use url::Url;

#[derive(Debug, Parser)]
#[command(
    name = "berlin-times",
    version,
    about = "Generate the Berlin Times edition"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Generate(GenerateArgs),
}

#[derive(Debug, clap::Args)]
struct GenerateArgs {
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    public_base_url: Url,
    #[arg(long)]
    fixture: Option<PathBuf>,
    #[arg(long, requires = "fixture")]
    fixture_image: Option<PathBuf>,
    #[arg(long)]
    at: Option<DateTime<Utc>>,
    #[arg(long, env = "EXA_API_BASE", default_value = "https://api.exa.ai/")]
    api_base: Url,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .without_time()
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Generate(args) => {
            let options = GenerateOptions {
                output: args.output,
                public_base_url: args.public_base_url,
                fixture: args.fixture,
                fixture_image: args.fixture_image,
                at: args.at.unwrap_or_else(Utc::now),
                api_key: std::env::var("EXA_API_KEY").ok(),
                api_base: args.api_base,
            };
            berlin_times::generate(&options)
                .await
                .context("edition generation failed")?;
        }
    }
    Ok(())
}
