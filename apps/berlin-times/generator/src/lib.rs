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
    exa::{ExaClient, fit_summary},
    model::{EditionV1, ResearchEdition},
    photo::{PhotoAsset, SafeHttp, fixture_photo},
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

struct PreviousEdition {
    edition: EditionV1,
    photo: Option<PreviousPhoto>,
}

struct PreviousPhoto {
    name: String,
    bytes: Vec<u8>,
    story_id: String,
    alt: String,
    credit: String,
    source_page_url: String,
}

impl PreviousPhoto {
    fn into_asset(self) -> (String, PhotoAsset) {
        (
            self.name,
            PhotoAsset {
                bytes: self.bytes,
                story_id: self.story_id,
                alt: self.alt,
                credit: self.credit,
                source_page_url: self.source_page_url,
            },
        )
    }
}

/// Generates and atomically publishes a complete static edition of The Berlin Times.
///
/// # Errors
///
/// Returns error when research, validation, photo processing, or publication fails.
pub async fn generate(options: &GenerateOptions) -> Result<EditionV1> {
    let mut previous = if options.fixture.is_some() {
        None
    } else {
        fetch_previous_edition(&options.public_base_url).await
    };
    let mut research = if let Some(path) = &options.fixture {
        read_fixture(path).await?
    } else {
        research_live(options, previous.as_ref().map(|previous| &previous.edition)).await?
    };
    validate_result(&research, options.at)?;

    let mut reused_photo_name = None;
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
        match SafeHttp::new().build_lead_photo(&research).await {
            Ok(photo) => photo,
            Err(error) => {
                let reused = previous.as_mut().and_then(|previous| {
                    take_reusable_previous_photo(&mut previous.photo, &research)
                });
                let Some((name, photo)) = reused else {
                    return Err(error);
                };
                reused_photo_name = Some(name);
                photo
            }
        }
    };

    fit_layout_summaries(&mut research);
    validate_result(&research, options.at)?;

    let (edition, photo_name) =
        site::assemble_edition(&research, &photo, &options.public_base_url, options.at)?;
    let previous_photo = if let Some(name) = reused_photo_name.as_deref() {
        Some((name, photo.bytes.as_slice()))
    } else {
        previous
            .as_ref()
            .and_then(|previous| previous.photo.as_ref())
            .map(|photo| (photo.name.as_str(), photo.bytes.as_slice()))
    };
    site::publish_site(
        &options.output,
        &edition,
        &photo_name,
        &photo.bytes,
        previous_photo,
    )?;
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
    client.research(options.at, previous).await
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

async fn fetch_previous_edition(public_base_url: &Url) -> Option<PreviousEdition> {
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
        Ok(response) if response.status().is_success() => {
            let edition = response.json::<EditionV1>().await.ok()?;
            let photo = fetch_previous_photo(public_base_url, &edition).await;
            Some(PreviousEdition { edition, photo })
        }
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

async fn fetch_previous_photo(public_base_url: &Url, edition: &EditionV1) -> Option<PreviousPhoto> {
    let name = previous_photo_name(public_base_url, &edition.lead_image.url)?;
    match SafeHttp::new()
        .fetch_published_photo(&edition.lead_image.url)
        .await
    {
        Ok(bytes) => Some(PreviousPhoto {
            name,
            bytes,
            story_id: edition.lead_image.story_id.clone(),
            alt: edition.lead_image.alt.clone(),
            credit: edition.lead_image.credit.clone(),
            source_page_url: edition.lead_image.source_page_url.clone(),
        }),
        Err(error) => {
            warn!(%error, "previous lead photo could not be retained");
            None
        }
    }
}

fn take_reusable_previous_photo(
    previous: &mut Option<PreviousPhoto>,
    research: &ResearchEdition,
) -> Option<(String, PhotoAsset)> {
    let story_id = previous.as_ref().map(|photo| photo.story_id.as_str())?;
    if !research.stories.iter().any(|story| story.id == story_id) {
        return None;
    }
    previous.take().map(PreviousPhoto::into_asset)
}

fn previous_photo_name(public_base_url: &Url, image_url: &str) -> Option<String> {
    let image_url = Url::parse(image_url).ok()?;
    if image_url.origin() != public_base_url.origin() {
        return None;
    }
    let mut prefix = String::from(public_base_url.path());
    if !prefix.ends_with('/') {
        prefix.push('/');
    }
    prefix.push_str("assets/");
    let name = image_url.path().strip_prefix(&prefix)?;
    let is_jpeg = Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jpg"));
    (name.starts_with("lead-") && is_jpeg && !name.contains('/')).then(|| name.into())
}

fn fit_layout_summaries(edition: &mut ResearchEdition) {
    edition
        .stories
        .iter_mut()
        .enumerate()
        .for_each(|(index, story)| {
            let limit = if index == 0 { 36 } else { 30 };
            story.summary = fit_summary(&story.summary, limit);
        });
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;
    use url::Url;

    use super::{
        GenerateOptions, PreviousPhoto, fit_layout_summaries, generate, previous_photo_name,
        take_reusable_previous_photo,
    };
    use crate::model::ResearchEdition;

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

    #[test]
    fn recognizes_previous_photo_on_public_site()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let base = Url::parse("https://example.com/trmnl-apps/")?;
        assert_eq!(
            previous_photo_name(
                &base,
                "https://example.com/trmnl-apps/assets/lead-20260806.jpg"
            )
            .as_deref(),
            Some("lead-20260806.jpg")
        );
        assert!(
            previous_photo_name(&base, "https://attacker.example/assets/lead-20260806.jpg")
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn reuses_cached_photo_only_for_a_selected_story()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let research = serde_json::from_str::<ResearchEdition>(include_str!(
            "../fixtures/valid-research.json"
        ))?;
        let story_id = research
            .stories
            .get(1)
            .map(|story| story.id.clone())
            .ok_or("fixture must contain second story")?;
        let bytes = vec![1, 2, 3, 4];
        let original_pointer = bytes.as_ptr();
        let mut cached = Some(PreviousPhoto {
            name: "lead-previous.jpg".into(),
            bytes,
            story_id: story_id.clone(),
            alt: "Cached alt".into(),
            credit: "Cached credit".into(),
            source_page_url: "https://www.ft.com/content/cached-photo".into(),
        });

        let (old_name, asset) =
            take_reusable_previous_photo(&mut cached, &research).ok_or("photo was not reused")?;
        assert_eq!(old_name, "lead-previous.jpg");
        assert_eq!(asset.story_id, story_id);
        assert_eq!(asset.bytes, [1, 2, 3, 4]);
        assert_eq!(asset.bytes.as_ptr(), original_pointer);
        assert!(cached.is_none());

        let mut unrelated = Some(PreviousPhoto {
            name: "lead-unrelated.jpg".into(),
            bytes: vec![5, 6, 7],
            story_id: "not-selected".into(),
            alt: "Unrelated alt".into(),
            credit: "Unrelated credit".into(),
            source_page_url: "https://www.reuters.com/world/unrelated/photo".into(),
        });
        assert!(take_reusable_previous_photo(&mut unrelated, &research).is_none());
        assert!(unrelated.is_some());
        Ok(())
    }

    #[test]
    fn fits_lead_and_supporting_summaries_without_reordering()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut edition = serde_json::from_str::<ResearchEdition>(include_str!(
            "../fixtures/valid-research.json"
        ))?;
        let summary = (0..40).map(|_| "word").collect::<Vec<_>>().join(" ");
        edition
            .stories
            .iter_mut()
            .for_each(|story| story.summary = format!("{summary}."));
        let lead_id = edition
            .stories
            .first()
            .map(|story| story.id.clone())
            .ok_or("fixture must contain a lead")?;
        let former_lead = edition
            .stories
            .get_mut(2)
            .ok_or("fixture must contain third story")?;
        former_lead.is_carried = true;

        fit_layout_summaries(&mut edition);

        assert_eq!(
            edition.stories.first().map(|story| &story.id),
            Some(&lead_id)
        );
        assert_eq!(
            edition
                .stories
                .first()
                .map(|story| story.summary.split_whitespace().count()),
            Some(36)
        );
        assert!(
            edition
                .stories
                .iter()
                .skip(1)
                .all(|story| story.summary.split_whitespace().count() <= 30)
        );
        assert_eq!(
            edition
                .stories
                .get(2)
                .map(|story| story.summary.split_whitespace().count()),
            Some(30)
        );
        Ok(())
    }
}
