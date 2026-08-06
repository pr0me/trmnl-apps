pub mod error;
mod exa;
pub mod model;
mod photo;
mod schedule;
mod site;
pub mod validate;

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Utc};
use tracing::{info, warn};
use url::Url;

use crate::{
    error::{GeneratorError, Result, io_error},
    exa::{ExaClient, normalize_and_select},
    model::{EditionV1, ResearchEdition},
    photo::{SafeHttp, fixture_photo},
};

pub struct GenerateOptions {
    pub output: PathBuf,
    pub public_base_url: Url,
    pub fixture: Option<PathBuf>,
    pub fixture_image: Option<PathBuf>,
    pub at: DateTime<Utc>,
    pub api_key: Option<String>,
    pub api_base: Url,
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
    let research = if let Some(path) = &options.fixture {
        read_fixture(path).await?
    } else {
        research_live(options, previous.as_ref()).await?
    };
    validate_result(&research, options.at)?;

    let photo = if let Some(path) = &options.fixture_image {
        let bytes = tokio::fs::read(path).await.map_err(io_error(path))?;
        let candidate = research
            .photo_candidates
            .first()
            .and_then(|id| research.stories.iter().find(|story| story.id == *id))
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
        SafeHttp::new().build_lead_photo(&research).await?
    };

    let (edition, photo_name) =
        site::assemble_edition(&research, &photo, &options.public_base_url, options.at)?;
    site::publish_site(&options.output, &edition, &photo_name, &photo.bytes)?;
    info!(edition_id = %edition.edition_id, output = %options.output.display(), "edition generated");
    Ok(edition)
}

async fn research_live(
    options: &GenerateOptions,
    previous: Option<&EditionV1>,
) -> Result<ResearchEdition> {
    let api_key = options
        .api_key
        .clone()
        .ok_or_else(|| GeneratorError::Config("exa_api_key is required".into()))?;
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?;
    let client = ExaClient::new(http, &options.api_base, api_key)?;
    let response = client.search(options.at).await?;
    normalize_and_select(response, options.at, previous)
}

fn validation_report(research: &ResearchEdition, at: DateTime<Utc>) -> validate::ValidationReport {
    validate::validate_edition(research, at)
}

fn validate_result(research: &ResearchEdition, at: DateTime<Utc>) -> Result<()> {
    validation_report(research, at).into_result()
}

async fn read_fixture(path: impl AsRef<Path>) -> Result<ResearchEdition> {
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

    use super::{GenerateOptions, generate};

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
            api_base: Url::parse("https://api.exa.ai/")?,
        };

        assert!(generate(&options).await.is_err());
        assert_eq!(tokio::fs::read(marker).await?, b"keep me");
        Ok(())
    }
}
