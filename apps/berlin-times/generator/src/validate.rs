use std::{collections::HashSet, hash::BuildHasher};

use chrono::{DateTime, Duration, Utc};
use url::Url;

use crate::{
    error::{GeneratorError, Result},
    model::{Category, ResearchEdition, ResearchSource, SourceTier},
};

const PREFERRED_DOMAINS: &str = include_str!("../config/preferred-domains.txt");
const OFFICIAL_DOMAINS: &str = include_str!("../config/official-domains.txt");

const TRACKING_PARAMETERS: &[&str] = &["fbclid", "gclid", "mc_cid", "mc_eid", "ref", "source"];

#[derive(Debug, Default, Eq, PartialEq)]
pub struct ValidationReport {
    pub problems: Vec<String>,
}

impl ValidationReport {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.problems.is_empty()
    }

    /// Converts report into successful validation or joined validation error.
    ///
    /// # Errors
    ///
    /// Returns validation error when report contains one or more problems.
    pub fn into_result(self) -> Result<()> {
        if self.is_valid() {
            Ok(())
        } else {
            Err(GeneratorError::Validation(self.problems.join("; ")))
        }
    }
}

#[must_use]
pub fn validate_edition(
    edition: &ResearchEdition,
    consulted_urls: &HashSet<String, impl BuildHasher>,
    now: DateTime<Utc>,
) -> ValidationReport {
    let mut report = ValidationReport::default();
    if edition.stories.len() != 6 {
        report.problems.push(format!(
            "expected exactly six stories, received {}",
            edition.stories.len()
        ));
    }

    validate_story_identity(edition, &mut report);
    validate_category_quotas(edition, &mut report);
    validate_photo_candidates(edition, &mut report);

    edition
        .stories
        .iter()
        .enumerate()
        .for_each(|(index, story)| {
            let prefix = format!("story {}", story.id);
            validate_plain_text(&story.headline, "headline", &prefix, &mut report);
            validate_plain_text(&story.summary, "summary", &prefix, &mut report);
            validate_copy(index, story, &prefix, &mut report);
            validate_freshness(story, now, &prefix, &mut report);
            validate_sources(story, consulted_urls, &prefix, &mut report);
        });

    report
}

/// Produces stable HTTPS source URL for comparison.
///
/// # Errors
///
/// Returns validation error when URL is invalid or does not use HTTPS.
pub fn canonicalize_url(value: &str) -> Result<String> {
    let mut parsed = Url::parse(value)
        .map_err(|error| GeneratorError::Validation(format!("invalid source url: {error}")))?;
    if parsed.scheme() != "https" {
        return Err(GeneratorError::Validation(
            "source url must use https".into(),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(GeneratorError::Validation(
            "source url must not contain credentials".into(),
        ));
    }
    if parsed.port_or_known_default() != Some(443) {
        return Err(GeneratorError::Validation(
            "source url must use the standard https port".into(),
        ));
    }
    parsed.set_fragment(None);
    if matches!(parsed.port(), Some(443)) {
        parsed
            .set_port(None)
            .map_err(|()| GeneratorError::Validation("invalid source url port".into()))?;
    }

    let mut pairs = parsed
        .query_pairs()
        .filter(|(key, _)| {
            let lower = key.to_ascii_lowercase();
            !lower.starts_with("utm_")
                && !TRACKING_PARAMETERS
                    .iter()
                    .any(|parameter| lower == *parameter)
        })
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    pairs.sort_unstable();
    parsed.set_query(None);
    if !pairs.is_empty() {
        parsed.query_pairs_mut().extend_pairs(pairs);
    }

    let path = parsed.path().trim_end_matches('/').to_owned();
    if !path.is_empty() {
        parsed.set_path(&path);
    }
    Ok(parsed.into())
}

#[must_use]
pub fn preferred_source_allowed(url: &str) -> bool {
    domain_allowed(url, PREFERRED_DOMAINS)
}

fn official_source_allowed(url: &str) -> bool {
    domain_allowed(url, OFFICIAL_DOMAINS)
}

fn domain_allowed(url: &str, allowlist: &str) -> bool {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| {
            allowlist
                .lines()
                .filter(|domain| !domain.is_empty())
                .any(|domain| host == domain || host.ends_with(&format!(".{domain}")))
        })
}

fn validate_story_identity(edition: &ResearchEdition, report: &mut ValidationReport) {
    let mut ids = HashSet::new();
    let mut events = HashSet::new();
    edition.stories.iter().for_each(|story| {
        if story.id.trim().is_empty() || !ids.insert(story.id.trim().to_ascii_lowercase()) {
            report
                .problems
                .push(format!("story id is empty or duplicated: {}", story.id));
        }
        let event = normalize_event_key(&story.event_key);
        if event.is_empty() || !events.insert(event) {
            report
                .problems
                .push(format!("event is empty or duplicated: {}", story.event_key));
        }
        if !story
            .qualifying_categories
            .contains(&story.primary_category)
        {
            report.problems.push(format!(
                "story {} primary category is not a qualifying category",
                story.id
            ));
        }
    });
}

fn validate_category_quotas(edition: &ResearchEdition, report: &mut ValidationReport) {
    [
        Category::GlobalPolitics,
        Category::GlobalEconomics,
        Category::Germany,
        Category::Technology,
    ]
    .iter()
    .filter(|category| {
        !edition
            .stories
            .iter()
            .any(|story| story.qualifying_categories.contains(category))
    })
    .for_each(|category| {
        report
            .problems
            .push(format!("missing required category: {category:?}"));
    });
    if !edition.stories.iter().any(|story| story.is_breaking) {
        report
            .problems
            .push("missing breaking or high-impact story".into());
    }
}

fn validate_photo_candidates(edition: &ResearchEdition, report: &mut ValidationReport) {
    let ids = edition
        .stories
        .iter()
        .map(|story| story.id.as_str())
        .collect::<HashSet<_>>();
    let ranked = edition
        .photo_candidates
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if edition.photo_candidates.len() != edition.stories.len()
        || ranked.len() != edition.photo_candidates.len()
        || ranked != ids
    {
        report
            .problems
            .push("photo candidates must rank every story exactly once".into());
    }
}

fn validate_plain_text(value: &str, field: &str, prefix: &str, report: &mut ValidationReport) {
    if value.trim().is_empty() || value.contains('<') || value.contains('>') {
        report
            .problems
            .push(format!("{prefix} {field} must be non-empty plain text"));
    }
}

fn validate_copy(
    index: usize,
    story: &crate::model::ResearchStory,
    prefix: &str,
    report: &mut ValidationReport,
) {
    let headline_words = word_count(&story.headline);
    if !(5..=12).contains(&headline_words) {
        report.problems.push(format!(
            "{prefix} headline has {headline_words} words; expected 5 to 12"
        ));
    }

    let summary_words = word_count(&story.summary);
    let expected = if index == 0 { 40..=60 } else { 28..=45 };
    if !expected.contains(&summary_words) {
        report.problems.push(format!(
            "{prefix} summary has {summary_words} words; expected {} to {}",
            expected.start(),
            expected.end()
        ));
    }

    let sentences = sentence_count(&story.summary);
    let max_sentences = if index == 0 { 3 } else { 2 };
    if sentences == 0 || sentences > max_sentences {
        report.problems.push(format!(
            "{prefix} summary has {sentences} sentences; expected 1 to {max_sentences}"
        ));
    }
}

fn validate_freshness(
    story: &crate::model::ResearchStory,
    now: DateTime<Utc>,
    prefix: &str,
    report: &mut ValidationReport,
) {
    let age = now.signed_duration_since(story.published_at);
    if age < Duration::minutes(-30) {
        report
            .problems
            .push(format!("{prefix} publication time is in the future"));
        return;
    }
    let extended = story.qualifying_categories.contains(&Category::Germany)
        || story.qualifying_categories.contains(&Category::Technology);
    let max_age = if extended {
        Duration::hours(72)
    } else {
        Duration::hours(36)
    };
    if age > max_age {
        report
            .problems
            .push(format!("{prefix} is too old at {} hours", age.num_hours()));
    }
}

fn validate_sources(
    story: &crate::model::ResearchStory,
    consulted_urls: &HashSet<String, impl BuildHasher>,
    prefix: &str,
    report: &mut ValidationReport,
) {
    if !(1..=3).contains(&story.sources.len()) {
        report
            .problems
            .push(format!("{prefix} must have one to three sources"));
    }
    let needs_two = story.is_breaking
        || story.qualifying_categories.iter().any(|category| {
            matches!(
                category,
                Category::GlobalPolitics | Category::GlobalEconomics
            )
        });
    if needs_two && story.sources.len() < 2 {
        report
            .problems
            .push(format!("{prefix} requires at least two sources"));
    }
    if !story
        .sources
        .iter()
        .any(|source| source.tier == SourceTier::Preferred)
    {
        report
            .problems
            .push(format!("{prefix} requires a preferred reporting source"));
    }

    let mut unique = HashSet::new();
    story.sources.iter().for_each(|source| {
        validate_source(source, consulted_urls, prefix, &mut unique, report);
    });
}

fn validate_source(
    source: &ResearchSource,
    consulted_urls: &HashSet<String, impl BuildHasher>,
    prefix: &str,
    unique: &mut HashSet<String>,
    report: &mut ValidationReport,
) {
    if source.name.trim().is_empty() {
        report
            .problems
            .push(format!("{prefix} has an unnamed source"));
    }
    let canonical = match canonicalize_url(&source.url) {
        Ok(value) => value,
        Err(error) => {
            report.problems.push(format!("{prefix}: {error}"));
            return;
        }
    };
    if !unique.insert(canonical.clone()) {
        report
            .problems
            .push(format!("{prefix} repeats source {canonical}"));
    }
    if !consulted_urls.contains(&canonical) {
        report.problems.push(format!(
            "{prefix} source was not returned by web search: {canonical}"
        ));
    }
    let allowed = match source.tier {
        SourceTier::Preferred => preferred_source_allowed(&source.url),
        SourceTier::OfficialPrimary => official_source_allowed(&source.url),
    };
    if !allowed {
        report.problems.push(format!(
            "{prefix} source does not match its configured tier: {}",
            source.url
        ));
    }
}

fn normalize_event_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric() || character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn word_count(value: &str) -> usize {
    value.split_whitespace().count()
}

fn sentence_count(value: &str) -> usize {
    value
        .split_inclusive(['.', '!', '?'])
        .filter(|sentence| word_count(sentence) >= 3)
        .count()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use chrono::{TimeZone, Utc};

    use super::{canonicalize_url, preferred_source_allowed, validate_edition};
    use crate::model::ResearchResult;

    #[test]
    fn canonicalizes_tracking_and_query_order() {
        let value =
            canonicalize_url("https://www.reuters.com/world/story/?utm_source=x&b=2&a=1#section");
        assert_eq!(
            value.ok().as_deref(),
            Some("https://www.reuters.com/world/story?a=1&b=2")
        );
    }

    #[test]
    fn applies_domain_boundaries_to_allowlist() {
        assert!(preferred_source_allowed("https://www.bbc.com/news/world"));
        assert!(!preferred_source_allowed(
            "https://bbc.com.attacker.example/news"
        ));
    }

    #[test]
    fn canonical_urls_can_be_compared_as_a_set() {
        let urls = [
            "https://apnews.com/article/example?utm_medium=social",
            "https://apnews.com/article/example",
        ]
        .iter()
        .filter_map(|url| canonicalize_url(url).ok())
        .collect::<HashSet<_>>();
        assert_eq!(urls.len(), 1);
    }

    #[test]
    fn rejects_credentials_and_nonstandard_ports() {
        assert!(canonicalize_url("https://user@reuters.com/world/story").is_err());
        assert!(canonicalize_url("https://reuters.com:8443/world/story").is_err());
    }

    #[test]
    fn valid_fixture_satisfies_editorial_contract()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fixture = serde_json::from_str::<ResearchResult>(include_str!(
            "../fixtures/valid-research.json"
        ))?;
        let consulted = fixture
            .consulted_sources
            .iter()
            .filter_map(|source| canonicalize_url(&source.url).ok())
            .collect::<HashSet<_>>();
        let now = Utc
            .with_ymd_and_hms(2026, 8, 5, 4, 15, 0)
            .single()
            .ok_or_else(|| std::io::Error::other("fixed time must exist"))?;
        let report = validate_edition(&fixture.edition, &consulted, now);
        assert!(report.is_valid(), "{}", report.problems.join("; "));
        Ok(())
    }

    #[test]
    fn rejects_duplicate_events_and_unknown_sources()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut fixture = serde_json::from_str::<ResearchResult>(include_str!(
            "../fixtures/valid-research.json"
        ))?;
        let duplicate = fixture
            .edition
            .stories
            .first()
            .map(|story| story.event_key.clone());
        if let (Some(duplicate), Some(story)) = (duplicate, fixture.edition.stories.get_mut(1)) {
            story.event_key = duplicate;
            if let Some(source) = story.sources.first_mut() {
                source.url = "https://example.com/unconsulted".into();
            }
        }
        let consulted = fixture
            .consulted_sources
            .iter()
            .filter_map(|source| canonicalize_url(&source.url).ok())
            .collect::<HashSet<_>>();
        let now = Utc
            .with_ymd_and_hms(2026, 8, 5, 4, 15, 0)
            .single()
            .ok_or_else(|| std::io::Error::other("fixed time must exist"))?;
        let report = validate_edition(&fixture.edition, &consulted, now);
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("duplicated"))
        );
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("configured tier"))
        );
        Ok(())
    }
}
