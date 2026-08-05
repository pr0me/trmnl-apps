pub mod error;
pub mod model;
mod openai;
mod photo;
mod schedule;
mod site;
pub mod validate;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Utc};
use tracing::{info, warn};
use url::Url;

use crate::{
    error::{GeneratorError, Result, io_error},
    model::{EditionV1, ResearchResult},
    openai::{OpenAiClient, ResearchContext},
    photo::{SafeHttp, fixture_photo},
};

pub const DEFAULT_MODEL: &str = "gpt-5.6-terra";

pub struct GenerateOptions {
    pub output: PathBuf,
    pub public_base_url: Url,
    pub fixture: Option<PathBuf>,
    pub fixture_image: Option<PathBuf>,
    pub at: DateTime<Utc>,
    pub api_key: Option<String>,
    pub api_base: Url,
    pub model: String,
}

/// Generates and atomically publishes complete Berlin Times static edition.
///
/// # Errors
///
/// Returns error when research, validation, photo processing, or publication fails.
pub async fn generate(options: &GenerateOptions) -> Result<EditionV1> {
    let previous = if options.fixture.is_some() {
        None
    } else {
        fetch_previous_edition(&options.public_base_url).await
    };
    let previous_headlines = previous
        .as_ref()
        .map(|edition| {
            edition
                .stories
                .iter()
                .map(|story| story.headline.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let research = if let Some(path) = &options.fixture {
        read_fixture(path).await?
    } else {
        research_live(options, &previous_headlines).await?
    };
    validate_result(&research, options.at)?;

    let photo = if let Some(path) = &options.fixture_image {
        let bytes = tokio::fs::read(path).await.map_err(io_error(path))?;
        let candidate = research
            .edition
            .photo_candidates
            .first()
            .and_then(|id| {
                research
                    .edition
                    .stories
                    .iter()
                    .find(|story| story.id == *id)
            })
            .ok_or_else(|| GeneratorError::Config("fixture has no photo candidate story".into()))?;
        let source = candidate
            .sources
            .first()
            .ok_or_else(|| GeneratorError::Config("fixture photo story has no source".into()))?;
        fixture_photo(&bytes, candidate, &source.name, &source.url)?
    } else if options.fixture.is_some() {
        return Err(GeneratorError::Config(
            "--fixture-image is required with --fixture".into(),
        ));
    } else {
        SafeHttp::new().build_lead_photo(&research.edition).await?
    };

    let (edition, photo_name) = site::assemble_edition(
        &research.edition,
        &photo,
        &options.public_base_url,
        options.at,
    )?;
    site::publish_site(&options.output, &edition, &photo_name, &photo.bytes)?;
    info!(edition_id = %edition.edition_id, output = %options.output.display(), "edition generated");
    Ok(edition)
}

async fn research_live(
    options: &GenerateOptions,
    previous_headlines: &[String],
) -> Result<ResearchResult> {
    let api_key = options
        .api_key
        .clone()
        .ok_or_else(|| GeneratorError::Config("OPENAI_API_KEY is required".into()))?;
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(180))
        .build()?;
    let client = OpenAiClient::new(http, &options.api_base, api_key, options.model.clone())?;
    let first = client
        .research(&ResearchContext {
            now: options.at,
            previous_headlines,
            validation_problems: &[],
        })
        .await?;
    let report = validation_report(&first, options.at);
    if report.is_valid() {
        return Ok(first);
    }
    warn!(problems = %report.problems.join("; "), "research failed semantic validation; retrying once");
    let second = client
        .research(&ResearchContext {
            now: options.at,
            previous_headlines,
            validation_problems: &report.problems,
        })
        .await?;
    validate_result(&second, options.at)?;
    Ok(second)
}

fn validation_report(research: &ResearchResult, at: DateTime<Utc>) -> validate::ValidationReport {
    let consulted = research
        .consulted_sources
        .iter()
        .filter_map(|source| validate::canonicalize_url(&source.url).ok())
        .collect::<HashSet<_>>();
    validate::validate_edition(&research.edition, &consulted, at)
}

fn validate_result(research: &ResearchResult, at: DateTime<Utc>) -> Result<()> {
    validation_report(research, at).into_result()
}

async fn read_fixture(path: impl AsRef<Path>) -> Result<ResearchResult> {
    let path = path.as_ref();
    let bytes = tokio::fs::read(path).await.map_err(io_error(path))?;
    serde_json::from_slice(&bytes).map_err(GeneratorError::from)
}

async fn fetch_previous_edition(public_base_url: &Url) -> Option<EditionV1> {
    let mut base = public_base_url.clone();
    if !base.path().ends_with('/') {
        let path = format!("{}/", base.path());
        base.set_path(&path);
    }
    let url = base.join("edition.json").ok()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .ok()?;
    match client.get(url).send().await {
        Ok(response) if response.status().is_success() => response.json().await.ok(),
        Ok(response) => {
            warn!(status = %response.status(), "previous edition was unavailable");
            None
        }
        Err(error) => {
            warn!(%error, "previous edition could not be fetched");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;
    use url::Url;

    use super::{DEFAULT_MODEL, GenerateOptions, generate};

    #[tokio::test]
    async fn failed_generation_preserves_existing_output()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let output = temporary.path().join("site");
        let marker = output.join("current-edition");
        tokio::fs::create_dir_all(&output).await?;
        tokio::fs::write(&marker, b"keep me").await?;
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/valid-research.json");
        let at = Utc
            .with_ymd_and_hms(2026, 8, 5, 4, 15, 0)
            .single()
            .ok_or_else(|| std::io::Error::other("fixed time must exist"))?;
        let options = GenerateOptions {
            output,
            public_base_url: Url::parse("https://example.com/berlin-times/")?,
            fixture: Some(fixture),
            fixture_image: None,
            at,
            api_key: None,
            api_base: Url::parse("https://api.openai.com/")?,
            model: DEFAULT_MODEL.into(),
        };

        assert!(generate(&options).await.is_err());
        assert_eq!(tokio::fs::read(marker).await?, b"keep me");
        Ok(())
    }
}
