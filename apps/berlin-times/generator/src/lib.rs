pub mod error;
mod exa;
pub mod model;
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
    exa::ExaClient,
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

/// Generates and atomically publishes a complete static edition of The Berlin Times.
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
    let (mut research, photo) = if let Some(path) = &options.fixture {
        let research = read_fixture(path).await?;
        let image_path = options.fixture_image.as_ref().ok_or_else(|| {
            GeneratorError::Config("--fixture-image is required with --fixture".into())
        })?;
        let bytes = tokio::fs::read(image_path)
            .await
            .map_err(io_error(image_path))?;
        let candidate = research
            .photo_candidates
            .iter()
            .filter_map(|id| research.stories.iter().find(|story| story.id == *id))
            .find(|story| !story.is_carried)
            .ok_or_else(|| {
                GeneratorError::Config("fixture has no fresh photo candidate story".into())
            })?;
        let source = candidate
            .sources
            .first()
            .ok_or_else(|| GeneratorError::Config("fixture photo story has no source".into()))?;
        let photo = fixture_photo(&bytes, candidate, &source.name, &source.url)?;
        (research, photo)
    } else {
        research_live(options, previous.as_ref()).await?
    };
    validate_result(&research, options.at)?;

    let rail_story_id = String::from(longest_summary_story_id_excluding(
        &research,
        &photo.story_id,
    )?);
    arrange_layout_stories(&mut research, &photo.story_id, &rail_story_id)?;
    info!(
        lead_story_id = %photo.story_id,
        rail_story_id = %rail_story_id,
        "edition layout roles assigned"
    );
    validate_result(&research, options.at)?;

    let (edition, photo_name) =
        site::assemble_edition(&research, &photo, &options.public_base_url, options.at)?;
    site::publish_site(&options.output, &edition, &photo_name, &photo.bytes)?;
    info!(edition_id = %edition.edition_id, output = %options.output.display(), "edition generated");
    Ok(edition)
}

async fn research_live(
    options: &GenerateOptions,
    previous: Option<&EditionV1>,
) -> Result<(ResearchEdition, PhotoAsset)> {
    let api_key = options
        .api_key
        .clone()
        .ok_or_else(|| GeneratorError::Config("exa_api_key is required".into()))?;
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?;
    let client = ExaClient::new(http, &options.api_base, api_key)?;
    let mut pool = client.research_pool(options.at, previous).await?;
    if let Err(error) = client.enrich_pool_images(&mut pool).await {
        warn!(%error, "Exa image metadata could not be resolved");
    }
    let candidates = pool.candidate_stories();
    let photos = SafeHttp::new().build_candidate_photos(&candidates).await?;
    let photographed_story_ids = photos
        .iter()
        .map(|photo| photo.story_id.clone())
        .collect::<HashSet<_>>();
    let (research, lead_story_id) = pool.finalize_with_photos(&photographed_story_ids)?;
    let photo = photos
        .into_iter()
        .find(|photo| photo.story_id == lead_story_id)
        .ok_or_else(|| {
            GeneratorError::NoPhoto("selected lead photograph was not retained".into())
        })?;
    Ok((research, photo))
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
        Ok(response) if response.status().is_success() => response.json::<EditionV1>().await.ok(),
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

fn longest_summary_story_id_excluding<'a>(
    edition: &'a ResearchEdition,
    excluded_story_id: &str,
) -> Result<&'a str> {
    edition
        .stories
        .iter()
        .filter(|story| story.id != excluded_story_id)
        .reduce(|longest, story| {
            if word_count(&longest.summary) >= word_count(&story.summary) {
                longest
            } else {
                story
            }
        })
        .map(|story| story.id.as_str())
        .ok_or_else(|| {
            GeneratorError::Validation("edition has no distinct story for right rail".into())
        })
}

fn arrange_layout_stories(
    edition: &mut ResearchEdition,
    lead_story_id: &str,
    rail_story_id: &str,
) -> Result<()> {
    if lead_story_id == rail_story_id {
        return Err(GeneratorError::Validation(
            "lead and right rail must use distinct stories".into(),
        ));
    }

    let mut lead = None;
    let mut rail = None;
    let mut supporting = Vec::new();
    for story in std::mem::take(&mut edition.stories) {
        if story.id == lead_story_id {
            lead = Some(story);
        } else if story.id == rail_story_id {
            rail = Some(story);
        } else {
            supporting.push(story);
        }
    }

    let lead = lead.ok_or_else(|| {
        GeneratorError::Validation("lead photograph does not match a selected story".into())
    })?;
    if lead.is_carried {
        return Err(GeneratorError::Validation(
            "lead photograph must belong to a fresh story".into(),
        ));
    }
    let rail = rail.ok_or_else(|| {
        GeneratorError::Validation("right rail does not match a selected story".into())
    })?;
    if supporting.len() != 2 {
        return Err(GeneratorError::Validation(
            "layout requires exactly two supporting stories".into(),
        ));
    }

    edition.stories = std::iter::once(lead)
        .chain(std::iter::once(rail))
        .chain(supporting)
        .collect();
    Ok(())
}

fn word_count(value: &str) -> usize {
    value.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;
    use url::Url;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::{
        GenerateOptions, arrange_layout_stories, generate, longest_summary_story_id_excluding,
        validate_result,
    };
    use crate::{
        exa::ExaClient,
        model::{EditionName, EditionV1, LeadImageV1, ResearchEdition, StoryV1},
        photo::fixture_photo,
    };

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

    #[tokio::test]
    async fn deep_contract_with_one_fresh_story_publishes_distinct_lead_and_rail()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let response = serde_json::from_str::<serde_json::Value>(include_str!(
            "../fixtures/exa-deep-response.json"
        ))?;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(3)
            .mount(&server)
            .await;
        let at = Utc
            .with_ymd_and_hms(2026, 8, 5, 10, 0, 0)
            .single()
            .ok_or("fixed time must exist")?;
        let carried = serde_json::from_str::<ResearchEdition>(include_str!(
            "../fixtures/valid-research.json"
        ))?;
        let previous_stories = carried
            .stories
            .iter()
            .map(StoryV1::from)
            .collect::<Vec<_>>();
        let previous_lead = previous_stories
            .first()
            .map(|story| story.id.clone())
            .ok_or("fixture must contain previous story")?;
        let previous = EditionV1 {
            schema_version: 1,
            edition_id: "previous-evening".into(),
            edition_name: EditionName::Evening,
            display_date: "Tuesday, 4 August 2026".into(),
            generated_at: "2026-08-04T16:00:00Z".parse()?,
            next_scheduled_at: "2026-08-05T06:00:00+02:00".parse()?,
            stories: previous_stories,
            lead_image: LeadImageV1 {
                story_id: previous_lead,
                url: "https://example.com/previous.jpg".into(),
                alt: "Previous lead".into(),
                credit: "Reuters".into(),
                source_page_url: "https://www.reuters.com/world/europe/previous-story".into(),
            },
        };
        let api_base = Url::parse(&format!("{}/", server.uri()))?;
        let client = ExaClient::new(reqwest::Client::new(), &api_base, "test-key")?;
        let mut research = client.research(at, Some(&previous)).await?;
        validate_result(&research, at)?;

        let fresh = research
            .stories
            .iter()
            .find(|story| !story.is_carried)
            .ok_or("deep response must yield one fresh story")?;
        assert!(fresh.image_url.is_none());
        let lead_id = fresh.id.clone();
        let source = fresh
            .sources
            .first()
            .ok_or("fresh story must have source")?;
        let photo = fixture_photo(
            include_bytes!("../fixtures/lead.ppm"),
            fresh,
            &source.name,
            &source.url,
        )?;
        let rail_id = String::from(longest_summary_story_id_excluding(
            &research,
            &photo.story_id,
        )?);
        arrange_layout_stories(&mut research, &photo.story_id, &rail_id)?;
        validate_result(&research, at)?;

        let public_base = Url::parse("https://example.com/berlin-times/")?;
        let (edition, photo_name) =
            crate::site::assemble_edition(&research, &photo, &public_base, at)?;
        let temporary = tempdir()?;
        let output = temporary.path().join("site");
        crate::site::publish_site(&output, &edition, &photo_name, &photo.bytes)?;
        let published = serde_json::from_slice::<EditionV1>(
            &tokio::fs::read(output.join("edition.json")).await?,
        )?;
        assert_eq!(edition.stories.len(), 4);
        assert_eq!(
            edition.stories.first().map(|story| &story.id),
            Some(&lead_id)
        );
        assert_eq!(edition.lead_image.story_id, lead_id);
        assert_ne!(
            edition.stories.get(1).map(|story| &story.id),
            Some(&lead_id)
        );
        assert_eq!(published.edition_id, edition.edition_id);
        assert_eq!(published.stories.len(), edition.stories.len());
        assert_eq!(published.lead_image.story_id, edition.lead_image.story_id);
        Ok(())
    }

    #[test]
    fn assigns_distinct_lead_rail_and_supporting_roles()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut edition = serde_json::from_str::<ResearchEdition>(include_str!(
            "../fixtures/valid-research.json"
        ))?;
        let original_ids = edition
            .stories
            .iter()
            .map(|story| story.id.clone())
            .collect::<Vec<_>>();
        let lead_id = original_ids.get(1).ok_or("fixture must contain lead")?;
        let rail_id = String::from(longest_summary_story_id_excluding(&edition, lead_id)?);

        arrange_layout_stories(&mut edition, lead_id, &rail_id)?;

        assert_eq!(edition.stories[0].id, *lead_id);
        assert_eq!(edition.stories[1].id, rail_id);
        assert_eq!(edition.stories[2].id, original_ids[2]);
        assert_eq!(edition.stories[3].id, original_ids[3]);
        assert!(!edition.stories[0].is_carried);
        Ok(())
    }

    #[test]
    fn longest_summary_ties_preserve_editorial_order()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut edition = serde_json::from_str::<ResearchEdition>(include_str!(
            "../fixtures/valid-research.json"
        ))?;
        edition
            .stories
            .iter_mut()
            .for_each(|story| story.summary = "same length".into());
        let excluded = edition
            .stories
            .first()
            .map(|story| story.id.clone())
            .ok_or("fixture must contain first story")?;
        let second_id = edition
            .stories
            .get(1)
            .map(|story| story.id.as_str())
            .ok_or("fixture must contain second story")?;

        assert_eq!(
            longest_summary_story_id_excluding(&edition, &excluded)?,
            second_id
        );
        Ok(())
    }

    #[test]
    fn rejects_colliding_or_carried_lead_roles()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let edition = serde_json::from_str::<ResearchEdition>(include_str!(
            "../fixtures/valid-research.json"
        ))?;
        let lead_id = edition
            .stories
            .get(1)
            .map(|story| story.id.clone())
            .ok_or("fixture must contain lead")?;
        let rail_id = String::from(longest_summary_story_id_excluding(&edition, &lead_id)?);
        let mut collision = edition.clone();
        assert!(arrange_layout_stories(&mut collision, &rail_id, &rail_id).is_err());

        let mut carried = edition;
        let carried_id = lead_id;
        let carried_story = carried
            .stories
            .get_mut(1)
            .ok_or("fixture must contain carried lead")?;
        carried_story.is_carried = true;
        assert!(arrange_layout_stories(&mut carried, &carried_id, &rail_id).is_err());
        Ok(())
    }

    #[test]
    fn layout_assignment_preserves_full_summaries()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut edition = serde_json::from_str::<ResearchEdition>(include_str!(
            "../fixtures/valid-research.json"
        ))?;
        let summary = (0..60).map(|_| "word").collect::<Vec<_>>().join(" ");
        edition
            .stories
            .iter_mut()
            .for_each(|story| story.summary = format!("{summary}."));
        let lead_id = edition
            .stories
            .first()
            .map(|story| story.id.clone())
            .ok_or("fixture must contain a lead")?;
        let rail_id = edition
            .stories
            .get(1)
            .map(|story| story.id.clone())
            .ok_or("fixture must contain a rail")?;
        arrange_layout_stories(&mut edition, &lead_id, &rail_id)?;

        assert_eq!(
            edition.stories.first().map(|story| &story.id),
            Some(&lead_id)
        );
        assert!(
            edition
                .stories
                .iter()
                .all(|story| story.summary.split_whitespace().count() == 60)
        );
        Ok(())
    }
}
