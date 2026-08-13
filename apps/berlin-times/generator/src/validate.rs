use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use url::Url;

use crate::{
    error::{GeneratorError, Result},
    model::{EditionName, ResearchEdition},
    schedule::berlin_day,
};

const TRACKING_PARAMETERS: &[&str] = &["fbclid", "gclid", "mc_cid", "mc_eid", "ref", "source"];

pub(crate) const MORNING_DOMAINS: &[&str] = &["wsj.com", "nytimes.com", "ft.com", "reuters.com"];
pub(crate) const EVENING_DOMAINS: &[&str] = &[
    "handelsblatt.com",
    "tagesschau.de",
    "ft.com",
    "dw.com",
    "bbc.com",
];

pub(crate) fn domains_for_edition(edition: &EditionName) -> &'static [&'static str] {
    match edition {
        EditionName::Morning => MORNING_DOMAINS,
        EditionName::Evening => EVENING_DOMAINS,
    }
}

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
pub fn validate_edition(edition: &ResearchEdition, now: DateTime<Utc>) -> ValidationReport {
    let mut report = ValidationReport::default();
    if edition.stories.len() != 4 {
        report.problems.push(format!(
            "expected exactly four stories, received {}",
            edition.stories.len()
        ));
    }
    if edition.stories.first().is_none_or(|story| story.is_carried) {
        report
            .problems
            .push("story 0 must be a fresh, non-carried story".into());
    }
    validate_story_identity(edition, &mut report);
    validate_photo_candidates(edition, &mut report);
    validate_provider_diversity(edition, &mut report);
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
            validate_source(story, &prefix, &mut report);
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
            "source url must use standard https port".into(),
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
pub fn article_url_allowed(value: &str) -> bool {
    canonicalize_url(value).is_ok_and(|canonical| {
        let Ok(url) = Url::parse(&canonical) else {
            return false;
        };
        let segments = url
            .path_segments()
            .into_iter()
            .flatten()
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>();
        let last_is_generic = segments.last().is_some_and(|segment| {
            matches!(
                segment.to_ascii_lowercase().as_str(),
                "business"
                    | "economy"
                    | "inland"
                    | "markets"
                    | "panorama"
                    | "politics"
                    | "search"
                    | "suche"
                    | "technology"
                    | "topic"
                    | "wirtschaft"
                    | "world"
            )
        });
        provider_name(&canonical).is_some() && segments.len() >= 2 && !last_is_generic
    })
}

#[must_use]
pub fn provider_name(value: &str) -> Option<&'static str> {
    let domain = provider_domain(value)?;
    PROVIDERS
        .iter()
        .find_map(|(candidate, name)| (*candidate == domain).then_some(*name))
}

pub(crate) fn provider_domain(value: &str) -> Option<&'static str> {
    let host = Url::parse(value).ok()?.host_str()?.to_ascii_lowercase();
    PROVIDERS.iter().find_map(|(domain, _)| {
        (host == *domain
            || host
                .strip_suffix(domain)
                .is_some_and(|prefix| prefix.ends_with('.')))
        .then_some(*domain)
    })
}

const PROVIDERS: &[(&str, &str)] = &[
    ("nytimes.com", "The New York Times"),
    ("ft.com", "Financial Times"),
    ("tagesschau.de", "Tagesschau"),
    ("reuters.com", "Reuters"),
    ("rbb24.de", "rbb24"),
    ("wsj.com", "The Wall Street Journal"),
    ("handelsblatt.com", "Handelsblatt"),
    ("dw.com", "DW"),
    ("bbc.com", "BBC"),
];

fn validate_story_identity(edition: &ResearchEdition, report: &mut ValidationReport) {
    let mut ids = HashSet::new();
    let mut urls = HashSet::new();
    edition.stories.iter().for_each(|story| {
        if story.id.trim().is_empty() || !ids.insert(story.id.trim().to_ascii_lowercase()) {
            report
                .problems
                .push(format!("story id is empty or duplicated: {}", story.id));
        }
        let canonical = story
            .sources
            .first()
            .and_then(|source| canonicalize_url(&source.url).ok());
        match canonical {
            Some(url) => {
                if !urls.insert(url) {
                    report
                        .problems
                        .push(format!("story {} repeats source url", story.id));
                }
            }
            None => {
                report
                    .problems
                    .push(format!("story {} lacks valid source url", story.id));
            }
        }
    });
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

fn validate_provider_diversity(edition: &ResearchEdition, report: &mut ValidationReport) {
    if edition.stories.len() != 4 {
        return;
    }
    let providers = edition.stories.iter().filter_map(|story| {
        let [source] = story.sources.as_slice() else {
            return None;
        };
        Some(source.name.as_str())
    });
    let mut counts = HashMap::<&str, usize>::new();
    providers.for_each(|provider| {
        *counts.entry(provider).or_default() += 1;
    });
    if counts
        .values()
        .copied()
        .max()
        .is_some_and(|count| count > 3)
    {
        report
            .problems
            .push("dominant source may supply at most three of four stories".into());
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
    let summary_words = word_count(&story.summary);
    let maximum = if index == 0 { 60 } else { 45 };
    if summary_words > maximum {
        report.problems.push(format!(
            "{prefix} summary has {summary_words} words; maximum is {maximum}"
        ));
    }
}

fn validate_freshness(
    story: &crate::model::ResearchStory,
    now: DateTime<Utc>,
    prefix: &str,
    report: &mut ValidationReport,
) {
    let latest = now + Duration::minutes(30);
    if story.published_at > latest {
        report
            .problems
            .push(format!("{prefix} publication time is in future"));
    }
    if story.is_carried {
        return;
    }

    let earliest = berlin_day(now)
        .and_then(|day| {
            day.start
                .checked_sub_signed(Duration::milliseconds(1))
                .ok_or_else(|| GeneratorError::Config("cannot calculate freshness boundary".into()))
        })
        .and_then(berlin_day)
        .map(|day| day.start);
    let Ok(earliest) = earliest else {
        report
            .problems
            .push("could not determine berlin freshness boundary".into());
        return;
    };
    if story.published_at < earliest {
        report
            .problems
            .push(format!("{prefix} is outside requested publication windows"));
    }
}

fn validate_source(
    story: &crate::model::ResearchStory,
    prefix: &str,
    report: &mut ValidationReport,
) {
    if story.sources.len() != 1 {
        report
            .problems
            .push(format!("{prefix} must have exactly one source"));
        return;
    }
    let Some(source) = story.sources.first() else {
        return;
    };
    if source.name.trim().is_empty() {
        report.problems.push(format!("{prefix} has unnamed source"));
    }
    if !article_url_allowed(&source.url) {
        report
            .problems
            .push(format!("{prefix} source is not allowed article url"));
    }
    if provider_name(&source.url) != Some(source.name.as_str()) {
        report
            .problems
            .push(format!("{prefix} source has incorrect provider name"));
    }
}

fn word_count(value: &str) -> usize {
    value.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::{
        EVENING_DOMAINS, MORNING_DOMAINS, article_url_allowed, canonicalize_url,
        domains_for_edition, provider_domain, provider_name, validate_edition,
    };
    use crate::model::{EditionName, ResearchEdition};

    fn fixture() -> std::result::Result<ResearchEdition, serde_json::Error> {
        serde_json::from_str(include_str!("../fixtures/valid-research.json"))
    }

    fn now() -> std::result::Result<DateTime<Utc>, Box<dyn std::error::Error>> {
        Utc.with_ymd_and_hms(2026, 8, 5, 4, 15, 0)
            .single()
            .ok_or_else(|| "fixed time must exist".into())
    }

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
    fn enforces_domain_boundaries_and_article_paths() {
        assert!(article_url_allowed(
            "https://www.reuters.com/world/europe/example"
        ));
        assert!(article_url_allowed(
            "https://www.bbc.com/news/articles/example"
        ));
        assert!(!article_url_allowed("https://reuters.com/world/"));
        assert!(!article_url_allowed(
            "https://reuters.com.attacker.example/world/story"
        ));
        assert!(!article_url_allowed(
            "https://bbc.com.attacker.example/news/articles/example"
        ));
        assert!(!article_url_allowed("http://reuters.com/world/story"));
    }

    #[test]
    fn maps_all_providers_deterministically() {
        let providers = [
            ("https://www.nytimes.com/a/b", "The New York Times"),
            ("https://www.ft.com/content/id", "Financial Times"),
            ("https://www.tagesschau.de/a/b", "Tagesschau"),
            ("https://www.reuters.com/a/b", "Reuters"),
            ("https://www.rbb24.de/a/b", "rbb24"),
            ("https://www.wsj.com/a/b", "The Wall Street Journal"),
            ("https://www.handelsblatt.com/a/b", "Handelsblatt"),
            ("https://www.dw.com/en/story/a-123", "DW"),
            ("https://m.dw.com/en/story/a-123", "DW"),
            ("https://www.bbc.com/news/articles/example", "BBC"),
        ];
        for (url, expected) in providers {
            assert_eq!(provider_name(url), Some(expected));
        }
        assert_eq!(provider_name("https://example.com/a/b"), None);
        assert_eq!(provider_domain("https://dw.com.attacker.example/a/b"), None);
        assert_eq!(provider_name("https://notdw.com/a/b"), None);
    }

    #[test]
    fn exposes_exact_edition_source_profiles() {
        assert_eq!(
            MORNING_DOMAINS,
            ["wsj.com", "nytimes.com", "ft.com", "reuters.com"]
        );
        assert_eq!(
            EVENING_DOMAINS,
            [
                "handelsblatt.com",
                "tagesschau.de",
                "ft.com",
                "dw.com",
                "bbc.com"
            ]
        );
        assert_eq!(domains_for_edition(&EditionName::Morning), MORNING_DOMAINS);
        assert_eq!(domains_for_edition(&EditionName::Evening), EVENING_DOMAINS);
    }
    #[test]
    fn valid_fixture_satisfies_published_contract()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let report = validate_edition(&fixture()?, now()?);
        assert!(report.is_valid(), "{}", report.problems.join("; "));
        Ok(())
    }

    #[test]
    fn accepts_prior_day_fresh_lead_and_arbitrarily_old_carry()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut research = fixture()?;
        let lead = research.stories.first_mut().ok_or("fixture has no lead")?;
        lead.published_at = "2026-08-03T23:00:00Z".parse()?;
        let carry = research.stories.last_mut().ok_or("fixture has no carry")?;
        carry.published_at = "2020-01-01T00:00:00Z".parse()?;
        carry.is_carried = true;

        let report = validate_edition(&research, now()?);
        assert!(report.is_valid(), "{}", report.problems.join("; "));
        Ok(())
    }

    #[test]
    fn rejects_carried_lead_and_future_timestamps()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut carried_lead = fixture()?;
        carried_lead
            .stories
            .first_mut()
            .ok_or("fixture has no lead")?
            .is_carried = true;
        let report = validate_edition(&carried_lead, now()?);
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem == "story 0 must be a fresh, non-carried story")
        );

        for carried in [false, true] {
            let mut research = fixture()?;
            let story = research.stories.get_mut(1).ok_or("fixture has no story")?;
            story.published_at = now()? + chrono::Duration::minutes(31);
            story.is_carried = carried;
            let report = validate_edition(&research, now()?);
            assert!(
                report
                    .problems
                    .iter()
                    .any(|problem| problem.contains("publication time is in future"))
            );
        }
        Ok(())
    }

    #[test]
    fn rejects_dominant_source_above_three_stories()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut research = fixture()?;
        research
            .stories
            .iter_mut()
            .take(4)
            .enumerate()
            .for_each(|(index, story)| {
                story.sources = vec![crate::model::ResearchSource {
                    name: "Reuters".into(),
                    url: format!("https://www.reuters.com/world/europe/report-{index}"),
                }];
            });

        let report = validate_edition(&research, now()?);
        assert!(report.problems.iter().any(|problem| {
            problem == "dominant source may supply at most three of four stories"
        }));
        Ok(())
    }
}
