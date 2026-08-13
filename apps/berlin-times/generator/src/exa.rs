use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fmt::Write as _,
    time::{Duration, SystemTime},
};

use chrono::{DateTime, SecondsFormat, Utc};
use reqwest::{StatusCode, header::RETRY_AFTER};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::time::sleep;
use tracing::info;
use url::Url;

use crate::{
    error::{GeneratorError, Result},
    model::{
        Category, EditionName, EditionV1, ResearchEdition, ResearchSource, ResearchStory, StoryV1,
    },
    schedule::{PublicationWindow, edition_name, prior_day_window, publication_window},
    validate::{
        article_url_allowed, canonicalize_url, domains_for_edition, provider_domain, provider_name,
    },
};

const MAX_ATTEMPTS: usize = 3;
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
const REQUIRED_STORIES: usize = 4;
const MAX_STORIES_PER_PROVIDER: usize = 3;
const MAX_SELECTION_CANDIDATES: usize = 18;
const GERMANY_TERMS: &[&str] = &[
    "berlin",
    "germany",
    "german",
    "deutschland",
    "deutsche",
    "bundestag",
    "bundesrat",
    "bundesregierung",
    "kanzler",
    "kanzlerin",
];
const TECHNOLOGY_TERMS: &[&str] = &[
    "ai",
    "ki",
    "software",
    "chip",
    "chips",
    "cyber",
    "digital",
    "technology",
    "technologie",
    "semiconductor",
    "internet",
];
const ECONOMICS_TERMS: &[&str] = &[
    "economy",
    "economic",
    "markets",
    "market",
    "inflation",
    "banking",
    "bank",
    "trade",
    "tariff",
    "tariffs",
    "earnings",
    "revenue",
    "orders",
    "finance",
    "wirtschaft",
    "märkte",
    "markt",
    "banken",
    "handel",
    "zölle",
    "umsatz",
    "aufträge",
    "finanzen",
];
const POLITICS_TERMS: &[&str] = &[
    "government",
    "election",
    "elections",
    "parliament",
    "minister",
    "ministers",
    "war",
    "sanctions",
    "diplomacy",
    "international",
    "regierung",
    "wahl",
    "wahlen",
    "parlament",
    "krieg",
    "sanktionen",
    "diplomatie",
];
const SECURITY_TERMS: &[&str] = &[
    "security",
    "military",
    "defence",
    "defense",
    "attack",
    "terror",
    "sicherheit",
    "militär",
    "angriff",
];
const CLIMATE_TERMS: &[&str] = &[
    "climate",
    "weather",
    "warming",
    "emissions",
    "klima",
    "wetter",
    "erwärmung",
    "emissionen",
];
const SCIENCE_TERMS: &[&str] = &[
    "science",
    "research",
    "scientist",
    "study",
    "wissenschaft",
    "forschung",
    "studie",
];

const MORNING_QUERY: &str = "Most important international news in global politics OR global economics OR security OR consequential technology";
const EVENING_QUERY: &str = "Today’s most important German and European news in politics OR economics OR security OR consequential technology";
pub const SUMMARY_PROMPT: &str = "As your first line, output `Title: {english_title}\\n`, providing translations for German titles.\nSummarize the news article in 2 short English sentences / 30-45 words. Make the first sentence self-contained and no longer than 30 words.\nDeliver the gist. Write the summary as it would appear in a newspaper itself; do not use \"Summary:\", \"The article explains\" or alike.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EditionProfile {
    query: &'static str,
    domains: &'static [&'static str],
}

fn edition_profile(edition: &EditionName) -> EditionProfile {
    EditionProfile {
        query: match edition {
            EditionName::Morning => MORNING_QUERY,
            EditionName::Evening => EVENING_QUERY,
        },
        domains: domains_for_edition(edition),
    }
}

struct SemanticAttempt<'a> {
    edition: &'a EditionName,
    number: usize,
    window_kind: &'static str,
    window: PublicationWindow,
    domains: &'a [&'static str],
    primary_window: bool,
}

#[derive(Clone)]
pub struct ExaClient {
    http: reqwest::Client,
    search_endpoint: Url,
    contents_endpoint: Url,
    api_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExaResponse {
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub cost_dollars: Option<CostDollars>,
    #[serde(default)]
    pub results: Vec<ExaResult>,
    #[serde(default)]
    output: Option<ExaOutput>,
}

#[derive(Debug, Deserialize)]
struct ExaOutput {
    content: ExaOutputContent,
    #[serde(default)]
    grounding: Vec<ExaGrounding>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ExaOutputContent {
    Structured(StructuredOutput),
    Json(String),
}

#[derive(Debug, Deserialize)]
struct StructuredOutput {
    #[serde(default)]
    stories: Vec<ExaResult>,
}

#[derive(Debug, Deserialize)]
struct ExaGrounding {
    #[serde(default)]
    citations: Vec<ExaCitation>,
}

#[derive(Debug, Deserialize)]
struct ExaCitation {
    url: String,
}

#[derive(Debug, Deserialize)]
pub struct CostDollars {
    #[serde(default)]
    pub total: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExaResult {
    #[serde(default)]
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub published_date: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
}

impl ExaResponse {
    fn research_results(&self) -> Vec<ExaResult> {
        let grounded_urls = self
            .output
            .iter()
            .flat_map(|output| &output.grounding)
            .flat_map(|grounding| &grounding.citations)
            .filter_map(|citation| canonicalize_url(&citation.url).ok())
            .collect::<HashSet<_>>();
        let raw_by_url = self
            .results
            .iter()
            .filter_map(|result| canonicalize_url(&result.url).ok().map(|url| (url, result)))
            .collect::<HashMap<_, _>>();
        let structured = self
            .output
            .as_ref()
            .and_then(|output| match &output.content {
                ExaOutputContent::Structured(content) => Some(content.stories.clone()),
                ExaOutputContent::Json(content) => {
                    serde_json::from_str::<StructuredOutput>(content)
                        .ok()
                        .map(|value| value.stories)
                }
            })
            .unwrap_or_default();
        let mut seen = HashSet::new();
        let mut results = structured
            .into_iter()
            .filter_map(|mut result| {
                let canonical = canonicalize_url(&result.url).ok()?;
                if !grounded_urls.contains(&canonical) || !seen.insert(canonical.clone()) {
                    return None;
                }
                result.image = None;
                if let Some(raw) = raw_by_url.get(&canonical) {
                    result.published_date =
                        result.published_date.or_else(|| raw.published_date.clone());
                    result.image.clone_from(&raw.image);
                }
                Some(result)
            })
            .collect::<Vec<_>>();
        results.extend(self.results.iter().filter_map(|result| {
            let canonical = canonicalize_url(&result.url).ok()?;
            seen.insert(canonical).then(|| result.clone())
        }));
        results
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchRequest<'a> {
    query: &'a str,
    category: &'a str,
    #[serde(rename = "type")]
    search_type: &'a str,
    num_results: usize,
    include_domains: &'a [&'static str],
    system_prompt: String,
    start_published_date: String,
    end_published_date: String,
    output_schema: Value,
    stream: bool,
    contents: Contents<'a>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Contents<'a> {
    summary: Summary<'a>,
    max_age_hours: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContentsRequest<'a> {
    urls: Vec<&'a str>,
    max_age_hours: i8,
}

#[derive(Debug, Deserialize)]
struct ContentsResponse {
    #[serde(default)]
    results: Vec<ContentsResult>,
}

#[derive(Debug, Deserialize)]
struct ContentsResult {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    image: Option<String>,
}

#[derive(Debug, Serialize)]
struct Summary<'a> {
    query: &'a str,
}

#[derive(Debug)]
struct Candidate {
    rank: (usize, usize),
    primary_window: bool,
    canonical_url: String,
    provider: &'static str,
    title: String,
    summary: String,
    published_at: DateTime<Utc>,
    image_url: Option<String>,
    categories: Vec<Category>,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct RejectionCounters {
    missing_or_invalid_copy: usize,
    missing_or_invalid_date: usize,
    outside_request_window: usize,
    disallowed_or_non_article_url: usize,
    duplicate_canonical_url: usize,
    repeated_previous_edition_url: usize,
}

#[derive(Debug)]
struct Normalization {
    candidates: Vec<Candidate>,
    rejections: RejectionCounters,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectionScore {
    primary_window: usize,
    required_categories: usize,
    providers: usize,
    has_image: bool,
    ranks: Vec<(usize, usize)>,
}

#[derive(Clone, Copy)]
struct SelectionTarget {
    novel: usize,
    carried: usize,
    require_provider_split: bool,
}

impl ExaClient {
    pub fn new(http: reqwest::Client, api_base: &Url, api_key: impl Into<String>) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(GeneratorError::Config(
                "exa_api_key must not be empty".into(),
            ));
        }
        let search_endpoint = api_base
            .join("search")
            .map_err(|error| GeneratorError::Config(format!("invalid api base url: {error}")))?;
        let contents_endpoint = api_base
            .join("contents")
            .map_err(|error| GeneratorError::Config(format!("invalid api base url: {error}")))?;
        Ok(Self {
            http,
            search_endpoint,
            contents_endpoint,
            api_key,
        })
    }

    pub async fn research(
        &self,
        now: DateTime<Utc>,
        previous: Option<&EditionV1>,
    ) -> Result<ResearchEdition> {
        let edition_name = edition_name(now);
        let profile = edition_profile(&edition_name);
        let previous_evening_at = previous
            .filter(|value| value.edition_name == EditionName::Evening)
            .map(|value| value.generated_at);
        let window = publication_window(now, previous_evening_at)?;
        let previous_urls = previous_urls(previous);

        let (initial, initial_normalization) = self
            .semantic_search(
                profile,
                SemanticAttempt {
                    edition: &edition_name,
                    number: 1,
                    window_kind: "primary",
                    window,
                    domains: profile.domains,
                    primary_window: true,
                },
                &previous_urls,
            )
            .await?;
        let mut candidates = initial_normalization.candidates;
        let mut responses = vec![initial];
        let mut current_domains = profile.domains.to_vec();

        if candidates.len() < REQUIRED_STORIES || select(&candidates, &[]).is_none() {
            let initial_results = responses[0].research_results();
            current_domains = retry_domains(
                profile.domains,
                profile.domains,
                &initial_results,
                &previous_urls,
            );
            let (retry, retry_normalization) = self
                .semantic_search(
                    profile,
                    SemanticAttempt {
                        edition: &edition_name,
                        number: 2,
                        window_kind: "primary",
                        window,
                        domains: &current_domains,
                        primary_window: true,
                    },
                    &previous_urls,
                )
                .await?;
            merge_candidates(&mut candidates, retry_normalization.candidates);
            responses.push(retry);
        }

        let valid_carries = carry_candidates(previous, now, &candidates);
        if candidates.is_empty()
            || candidates.len() + valid_carries.len() < REQUIRED_STORIES
            || select(&candidates, &valid_carries).is_none()
        {
            let combined_results = responses
                .iter()
                .flat_map(ExaResponse::research_results)
                .collect::<Vec<_>>();
            let prior_domains = retry_domains(
                profile.domains,
                &current_domains,
                &combined_results,
                &previous_urls,
            );
            let prior_window = prior_day_window(window)?;
            let (prior, prior_normalization) = self
                .semantic_search(
                    profile,
                    SemanticAttempt {
                        edition: &edition_name,
                        number: 3,
                        window_kind: "prior_day",
                        window: prior_window,
                        domains: &prior_domains,
                        primary_window: false,
                    },
                    &previous_urls,
                )
                .await?;
            merge_candidates(&mut candidates, prior_normalization.candidates);
            responses.push(prior);
        }

        finalize_research(candidates, previous, now, &responses)
    }

    pub async fn enrich_images(&self, edition: &mut ResearchEdition) -> Result<usize> {
        let urls = edition
            .stories
            .iter()
            .filter(|story| !story.is_carried)
            .filter_map(|story| story.sources.first())
            .map(|source| source.url.as_str())
            .collect::<Vec<_>>();
        if urls.is_empty() {
            return Ok(0);
        }
        let requested = urls.len();
        let response = self
            .send_with_retry::<ContentsResponse>(
                &self.contents_endpoint,
                &ContentsRequest {
                    urls,
                    max_age_hours: -1,
                },
            )
            .await?;
        let images = response
            .results
            .into_iter()
            .filter_map(|result| {
                let source_url = result.url.or(result.id)?;
                let canonical = canonicalize_url(&source_url).ok()?;
                let image = result.image.filter(|value| safe_image_url(value))?;
                Some((canonical, image))
            })
            .collect::<HashMap<_, _>>();
        let mut enriched = 0;
        edition
            .stories
            .iter_mut()
            .filter(|story| !story.is_carried)
            .for_each(|story| {
                let image = story
                    .sources
                    .first()
                    .and_then(|source| canonicalize_url(&source.url).ok())
                    .and_then(|url| images.get(&url));
                if let Some(image) = image {
                    story.image_url = Some(image.clone());
                    enriched += 1;
                }
            });
        info!(requested, enriched, "Exa image metadata resolved");
        Ok(enriched)
    }

    async fn semantic_search(
        &self,
        profile: EditionProfile,
        attempt: SemanticAttempt<'_>,
        previous_urls: &HashSet<String>,
    ) -> Result<(ExaResponse, Normalization)> {
        let body = search_request(profile, attempt.window, attempt.domains);
        let response = self
            .send_with_retry::<ExaResponse>(&self.search_endpoint, &body)
            .await?;
        let results = response.research_results();
        let normalization = normalize(
            &results,
            attempt.window,
            attempt.domains,
            previous_urls,
            attempt.number.saturating_sub(1),
            attempt.primary_window,
        );
        log_search_attempt(
            attempt.edition,
            attempt.number,
            attempt.window_kind,
            attempt.window,
            attempt.domains,
            &response,
            &normalization,
        );
        Ok((response, normalization))
    }

    async fn send_with_retry<Response>(
        &self,
        endpoint: &Url,
        body: &impl Serialize,
    ) -> Result<Response>
    where
        Response: DeserializeOwned,
    {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let response = self
                .http
                .post(endpoint.clone())
                .bearer_auth(&self.api_key)
                .json(body)
                .send()
                .await;

            match response {
                Ok(response) if response.status().is_success() => {
                    return response.json().await.map_err(GeneratorError::from);
                }
                Ok(response) => {
                    let status = response.status();
                    let delay = retry_delay(&response, attempt);
                    let body = response.text().await.ok();
                    let error = parse_error_response(status, body.as_deref());
                    if attempt < MAX_ATTEMPTS && is_transient_status(status) {
                        sleep(delay).await;
                        continue;
                    }
                    return Err(error);
                }
                Err(error) => {
                    if attempt < MAX_ATTEMPTS && (error.is_timeout() || error.is_connect()) {
                        sleep(backoff(attempt)).await;
                        continue;
                    }
                    return Err(GeneratorError::Request(error));
                }
            }
        }
    }
}

fn finalize_research(
    mut candidates: Vec<Candidate>,
    previous: Option<&EditionV1>,
    now: DateTime<Utc>,
    responses: &[ExaResponse],
) -> Result<ResearchEdition> {
    if candidates.is_empty() {
        return Err(GeneratorError::Validation(
            "exa returned no novel usable stories after the prior-day fallback; refusing to publish without a fresh lead"
                .into(),
        ));
    }
    order_and_limit_candidates(&mut candidates);
    let carried = carry_candidates(previous, now, &candidates);
    let carried_count = carried.len();
    if candidates.len() + carried_count < REQUIRED_STORIES {
        return Err(GeneratorError::Validation(format!(
            "only {} novel and {carried_count} carried stories were usable; four are required",
            candidates.len()
        )));
    }
    let (selected, carried_selected) = select(&candidates, &carried).ok_or_else(|| {
        GeneratorError::Validation(
            "could not select four stories from at least two providers".into(),
        )
    })?;
    let selected_novel = selected.len();
    let selected_carried = carried_selected.len();
    let mut stories = build_stories(selected);
    stories.extend(carried_selected.into_iter().cloned());
    let research = ResearchEdition {
        photo_candidates: photo_candidates(&stories),
        stories,
    };
    log_selection(&research, responses, selected_novel, selected_carried);
    Ok(research)
}

fn search_request<'a>(
    profile: EditionProfile,
    window: PublicationWindow,
    include_domains: &'a [&'static str],
) -> SearchRequest<'a> {
    let requested_sources = 3.min(include_domains.len());
    SearchRequest {
        query: profile.query,
        category: "news",
        search_type: "deep",
        num_results: 10,
        include_domains,
        system_prompt: format!(
            "Return up to 10 distinct individual news articles published inside the requested date range and grounded in exact article URLs from the allowed domains. Prefer {requested_sources} distinct sources when available, but return relevant articles when fewer are available. Avoid duplicate events and prefer consequential current reporting over analysis or opinion. Write every title and summary in English, translating German titles. Summaries must contain 2 short sentences and 30-45 words, must not repeat the title, and must not use labels such as `Title:` or `Summary:`."
        ),
        start_published_date: window.start.to_rfc3339_opts(SecondsFormat::Millis, true),
        end_published_date: window
            .end
            .checked_sub_signed(chrono::Duration::milliseconds(1))
            .unwrap_or(window.end)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
        output_schema: story_output_schema(),
        stream: false,
        contents: Contents {
            summary: Summary {
                query: SUMMARY_PROMPT,
            },
            max_age_hours: 0,
        },
    }
}

fn story_output_schema() -> Value {
    json!({
        "type": "object",
        "required": ["stories"],
        "properties": {
            "stories": {
                "type": "array",
                "maxItems": 10,
                "items": {
                    "type": "object",
                    "required": ["title", "url", "publishedDate", "summary"],
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "Concise English newspaper headline; translate German source titles"
                        },
                        "url": {
                            "type": "string",
                            "description": "Exact canonical URL of the individual source article"
                        },
                        "publishedDate": {
                            "type": "string",
                            "description": "Article publication timestamp in ISO 8601 format"
                        },
                        "summary": {
                            "type": "string",
                            "description": "Two short English sentences totaling 30-45 words without repeating the headline"
                        }
                    }
                }
            }
        }
    })
}

fn normalize(
    results: &[ExaResult],
    window: PublicationWindow,
    include_domains: &[&'static str],
    previous_urls: &HashSet<String>,
    attempt_index: usize,
    primary_window: bool,
) -> Normalization {
    let mut urls = HashSet::new();
    let mut candidates = Vec::<Candidate>::new();
    let mut rejections = RejectionCounters::default();

    for (rank, result) in results.iter().enumerate() {
        let Some(raw_summary) = result.summary.as_deref() else {
            rejections.missing_or_invalid_copy += 1;
            continue;
        };
        let (title, summary) = title_and_summary(&result.title, raw_summary);
        if title.is_empty()
            || title.contains('<')
            || title.contains('>')
            || summary.is_empty()
            || summary.contains('<')
            || summary.contains('>')
        {
            rejections.missing_or_invalid_copy += 1;
            continue;
        }
        let Some(published_at) = result
            .published_date
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
        else {
            rejections.missing_or_invalid_date += 1;
            continue;
        };
        if published_at < window.start || published_at >= window.end {
            rejections.outside_request_window += 1;
            continue;
        }
        if !article_url_allowed(&result.url) {
            rejections.disallowed_or_non_article_url += 1;
            continue;
        }
        let Ok(canonical_url) = canonicalize_url(&result.url) else {
            rejections.disallowed_or_non_article_url += 1;
            continue;
        };
        let Some(domain) = provider_domain(&canonical_url) else {
            rejections.disallowed_or_non_article_url += 1;
            continue;
        };
        if !include_domains.contains(&domain) {
            rejections.disallowed_or_non_article_url += 1;
            continue;
        }
        if previous_urls.contains(&canonical_url) {
            rejections.repeated_previous_edition_url += 1;
            continue;
        }
        if !urls.insert(canonical_url.clone()) {
            rejections.duplicate_canonical_url += 1;
            continue;
        }
        let Some(provider) = provider_name(&canonical_url) else {
            rejections.disallowed_or_non_article_url += 1;
            continue;
        };
        let categories = classify(&canonical_url, &title, &summary, provider);
        let image_url = result
            .image
            .as_ref()
            .filter(|value| safe_image_url(value))
            .cloned();
        candidates.push(Candidate {
            rank: (attempt_index, rank),
            primary_window,
            canonical_url,
            provider,
            title,
            summary,
            published_at,
            image_url,
            categories,
        });
    }
    Normalization {
        candidates,
        rejections,
    }
}

fn merge_candidates(candidates: &mut Vec<Candidate>, additional: Vec<Candidate>) {
    let mut urls = candidates
        .iter()
        .map(|candidate| candidate.canonical_url.clone())
        .collect::<HashSet<_>>();
    candidates.extend(
        additional
            .into_iter()
            .filter(|candidate| urls.insert(candidate.canonical_url.clone())),
    );
}

fn order_and_limit_candidates(candidates: &mut Vec<Candidate>) {
    candidates.sort_by_key(|candidate| (!candidate.primary_window, candidate.rank));
    candidates.truncate(MAX_SELECTION_CANDIDATES);
}

fn retry_domains(
    profile_domains: &[&'static str],
    current_domains: &[&'static str],
    results: &[ExaResult],
    previous_urls: &HashSet<String>,
) -> Vec<&'static str> {
    if current_domains.len() <= 2 {
        return current_domains.to_vec();
    }

    let mut counts = profile_domains
        .iter()
        .map(|domain| (*domain, 0_usize))
        .collect::<std::collections::HashMap<_, _>>();
    let mut stale_domains = HashSet::new();
    for result in results {
        let Some(domain) =
            provider_domain(&result.url).filter(|domain| counts.contains_key(domain))
        else {
            continue;
        };
        if let Some(count) = counts.get_mut(domain) {
            *count += 1;
        }
        if canonicalize_url(&result.url).is_ok_and(|url| previous_urls.contains(&url)) {
            stale_domains.insert(domain);
        }
    }

    let profile_index = |domain: &str| {
        profile_domains
            .iter()
            .position(|candidate| *candidate == domain)
            .unwrap_or(profile_domains.len())
    };
    let mut remaining = current_domains.to_vec();
    let mut stale = remaining
        .iter()
        .copied()
        .filter(|domain| stale_domains.contains(domain))
        .collect::<Vec<_>>();
    stale.sort_by(|left, right| {
        counts
            .get(right)
            .copied()
            .unwrap_or_default()
            .cmp(&counts.get(left).copied().unwrap_or_default())
            .then_with(|| profile_index(left).cmp(&profile_index(right)))
    });
    for domain in stale {
        if remaining.len() == 2 {
            break;
        }
        remaining.retain(|candidate| *candidate != domain);
    }

    if remaining.len() == current_domains.len() {
        let remove = remaining.iter().copied().max_by(|left, right| {
            counts
                .get(left)
                .copied()
                .unwrap_or_default()
                .cmp(&counts.get(right).copied().unwrap_or_default())
                .then_with(|| profile_index(right).cmp(&profile_index(left)))
        });
        if let Some(remove) = remove {
            remaining.retain(|candidate| *candidate != remove);
        }
    }

    remaining
}

fn log_search_attempt(
    edition: &EditionName,
    attempt: usize,
    window_kind: &str,
    window: PublicationWindow,
    domains: &[&str],
    response: &ExaResponse,
    normalization: &Normalization,
) {
    let rejections = &normalization.rejections;
    info!(
        edition = edition.as_str(),
        attempt,
        window_kind,
        window_start = %window.start.to_rfc3339_opts(SecondsFormat::Millis, true),
        window_end = %window.end.to_rfc3339_opts(SecondsFormat::Millis, true),
        domains = %domains.join(","),
        returned = response.research_results().len(),
        usable = normalization.candidates.len(),
        novel = normalization.candidates.len(),
        missing_or_invalid_copy = rejections.missing_or_invalid_copy,
        missing_or_invalid_date = rejections.missing_or_invalid_date,
        outside_request_window = rejections.outside_request_window,
        disallowed_or_non_article_url = rejections.disallowed_or_non_article_url,
        duplicate_canonical_url = rejections.duplicate_canonical_url,
        repeated_previous_edition_url = rejections.repeated_previous_edition_url,
        request_id = response.request_id.as_deref().unwrap_or("unavailable"),
        cost_dollars = ?response.cost_dollars.as_ref().and_then(|cost| cost.total),
        "Exa search attempt normalized"
    );
}

fn log_selection(
    research: &ResearchEdition,
    responses: &[ExaResponse],
    selected_novel: usize,
    selected_carried: usize,
) {
    let mut providers = research
        .stories
        .iter()
        .filter_map(|story| story.sources.first().map(|source| source.name.as_str()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    providers.sort_unstable();
    let mut categories = research
        .stories
        .iter()
        .map(|story| format!("{:?}", story.primary_category))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    categories.sort_unstable();
    let request_ids = responses
        .iter()
        .filter_map(|response| response.request_id.as_deref())
        .collect::<Vec<_>>()
        .join(",");
    let cost = responses
        .iter()
        .filter_map(|response| response.cost_dollars.as_ref()?.total)
        .sum::<f64>();
    info!(
        request_ids,
        selected_novel,
        selected_carried,
        provider_coverage = %providers.join(","),
        category_coverage = %categories.join(","),
        cost_dollars = cost,
        lead_fresh = true,
        "Exa stories selected"
    );
}

fn select<'a>(
    candidates: &'a [Candidate],
    carried: &'a [ResearchStory],
) -> Option<(Vec<&'a Candidate>, Vec<&'a ResearchStory>)> {
    let target = REQUIRED_STORIES.min(candidates.len() + carried.len());
    let maximum_novel = target.min(candidates.len());
    for novel_target in (1..=maximum_novel).rev() {
        let carry_target = target.saturating_sub(novel_target);
        if carry_target > carried.len() {
            continue;
        }
        let mut best = None::<(SelectionScore, Vec<&'a Candidate>, Vec<&'a ResearchStory>)>;
        let mut current = Vec::with_capacity(novel_target);
        enumerate_novel_subsets(
            candidates,
            carried,
            SelectionTarget {
                novel: novel_target,
                carried: carry_target,
                require_provider_split: target == REQUIRED_STORIES,
            },
            0,
            &mut current,
            &mut best,
        );
        if let Some((_, novel, carried)) = best {
            return Some((novel, carried));
        }
    }
    None
}

fn enumerate_novel_subsets<'a>(
    candidates: &'a [Candidate],
    carried: &'a [ResearchStory],
    target: SelectionTarget,
    start: usize,
    current: &mut Vec<&'a Candidate>,
    best: &mut Option<(SelectionScore, Vec<&'a Candidate>, Vec<&'a ResearchStory>)>,
) {
    if current.len() == target.novel {
        let Some(selected_carries) = select_carries(
            current,
            carried,
            target.carried,
            target.require_provider_split,
        ) else {
            return;
        };
        let score = selection_score(current);
        if best
            .as_ref()
            .is_none_or(|(best_score, _, _)| score.better_than(best_score))
        {
            *best = Some((score, current.clone(), selected_carries));
        }
        return;
    }
    let needed = target.novel.saturating_sub(current.len());
    if candidates.len().saturating_sub(start) < needed {
        return;
    }
    (start..candidates.len()).for_each(|index| {
        current.push(&candidates[index]);
        enumerate_novel_subsets(candidates, carried, target, index + 1, current, best);
        let _removed = current.pop();
    });
}

fn select_carries<'a>(
    novel: &[&Candidate],
    carried: &'a [ResearchStory],
    target: usize,
    require_provider_split: bool,
) -> Option<Vec<&'a ResearchStory>> {
    let mut current = Vec::with_capacity(target);
    find_carry_subset(
        novel,
        carried,
        target,
        require_provider_split,
        0,
        &mut current,
    )
}

fn find_carry_subset<'a>(
    novel: &[&Candidate],
    carried: &'a [ResearchStory],
    target: usize,
    require_provider_split: bool,
    start: usize,
    current: &mut Vec<&'a ResearchStory>,
) -> Option<Vec<&'a ResearchStory>> {
    if current.len() == target {
        return (!require_provider_split || provider_split_allowed(novel, current))
            .then(|| current.clone());
    }
    let needed = target.saturating_sub(current.len());
    if carried.len().saturating_sub(start) < needed {
        return None;
    }
    (start..carried.len()).find_map(|index| {
        current.push(&carried[index]);
        let selected = find_carry_subset(
            novel,
            carried,
            target,
            require_provider_split,
            index + 1,
            current,
        );
        let _removed = current.pop();
        selected
    })
}

fn provider_split_allowed(novel: &[&Candidate], carried: &[&ResearchStory]) -> bool {
    let providers = novel.iter().map(|candidate| candidate.provider).chain(
        carried
            .iter()
            .filter_map(|story| story.sources.first().map(|source| source.name.as_str())),
    );
    let mut counts = HashMap::<&str, usize>::new();
    let mut total = 0_usize;
    providers.for_each(|provider| {
        *counts.entry(provider).or_default() += 1;
        total += 1;
    });
    total == REQUIRED_STORIES
        && counts
            .values()
            .all(|count| *count <= MAX_STORIES_PER_PROVIDER)
}

fn selection_score(candidates: &[&Candidate]) -> SelectionScore {
    let required_categories = [
        Category::Germany,
        Category::Technology,
        Category::GlobalEconomics,
        Category::GlobalPolitics,
    ]
    .iter()
    .filter(|category| {
        candidates
            .iter()
            .any(|candidate| candidate.categories.contains(category))
    })
    .count();
    let providers = candidates
        .iter()
        .map(|candidate| candidate.provider)
        .collect::<HashSet<_>>()
        .len()
        .min(3);
    SelectionScore {
        primary_window: candidates
            .iter()
            .filter(|candidate| candidate.primary_window)
            .count(),
        required_categories,
        providers,
        has_image: candidates
            .iter()
            .any(|candidate| candidate.image_url.is_some()),
        ranks: candidates.iter().map(|candidate| candidate.rank).collect(),
    }
}

impl SelectionScore {
    fn better_than(&self, other: &Self) -> bool {
        self.primary_window
            .cmp(&other.primary_window)
            .then_with(|| self.required_categories.cmp(&other.required_categories))
            .then_with(|| self.providers.cmp(&other.providers))
            .then_with(|| self.has_image.cmp(&other.has_image))
            .then_with(|| other.ranks.cmp(&self.ranks))
            == Ordering::Greater
    }
}

fn build_stories(selected: Vec<&Candidate>) -> Vec<ResearchStory> {
    selected
        .into_iter()
        .map(|candidate| ResearchStory {
            id: story_id(&candidate.title, &candidate.canonical_url),
            primary_category: candidate
                .categories
                .first()
                .cloned()
                .unwrap_or(Category::World),
            is_developing: false,
            headline: candidate.title.clone(),
            summary: fit_summary(&candidate.summary, 45),
            published_at: candidate.published_at,
            sources: vec![ResearchSource {
                name: candidate.provider.into(),
                url: candidate.canonical_url.clone(),
            }],
            image_url: candidate.image_url.clone(),
            is_carried: false,
        })
        .collect()
}

fn photo_candidates(stories: &[ResearchStory]) -> Vec<String> {
    stories
        .iter()
        .filter(|story| story.image_url.is_some())
        .chain(stories.iter().filter(|story| story.image_url.is_none()))
        .map(|story| story.id.clone())
        .collect()
}

fn previous_urls(previous: Option<&EditionV1>) -> HashSet<String> {
    previous
        .into_iter()
        .flat_map(|edition| &edition.stories)
        .flat_map(|story| &story.sources)
        .filter_map(|source| canonicalize_url(&source.url).ok())
        .collect()
}

fn carried_story(story: &StoryV1, now: DateTime<Utc>) -> Option<ResearchStory> {
    if !plain_text_present(&story.id)
        || !plain_text_present(&story.headline)
        || !plain_text_present(&story.summary)
    {
        return None;
    }
    let [source] = story.sources.as_slice() else {
        return None;
    };
    if !article_url_allowed(&source.url) || provider_name(&source.url)? != source.name {
        return None;
    }
    let latest = now
        .checked_add_signed(chrono::Duration::minutes(30))
        .unwrap_or(now);
    if story.published_at > latest {
        return None;
    }

    Some(ResearchStory {
        id: story.id.clone(),
        primary_category: story.primary_category.clone(),
        is_developing: story.is_developing,
        headline: story.headline.clone(),
        summary: story.summary.clone(),
        published_at: story.published_at,
        sources: vec![ResearchSource {
            name: source.name.clone(),
            url: source.url.clone(),
        }],
        image_url: None,
        is_carried: true,
    })
}

fn carry_candidates(
    previous: Option<&EditionV1>,
    now: DateTime<Utc>,
    candidates: &[Candidate],
) -> Vec<ResearchStory> {
    let mut urls = candidates
        .iter()
        .map(|candidate| candidate.canonical_url.clone())
        .collect::<HashSet<_>>();
    let mut ids = candidates
        .iter()
        .map(|candidate| story_id(&candidate.title, &candidate.canonical_url))
        .collect::<HashSet<_>>();
    let mut carried = Vec::new();

    for story in previous.into_iter().flat_map(|edition| &edition.stories) {
        let Some(candidate) = carried_story(story, now) else {
            continue;
        };
        let Some(url) = candidate
            .sources
            .first()
            .and_then(|source| canonicalize_url(&source.url).ok())
        else {
            continue;
        };
        if urls.contains(&url) || ids.contains(&candidate.id) {
            continue;
        }
        urls.insert(url);
        ids.insert(candidate.id.clone());
        carried.push(candidate);
    }
    carried
}

fn plain_text_present(value: &str) -> bool {
    !value.trim().is_empty() && !value.contains(['<', '>'])
}

fn classify(url: &str, title: &str, summary: &str, provider: &str) -> Vec<Category> {
    let path = Url::parse(url)
        .ok()
        .map_or_else(String::new, |value| value.path().to_ascii_lowercase());
    let joined = format!("{path} {title} {summary}").to_lowercase();
    let signals = normalized_words(&joined);
    let mut categories = Vec::new();
    if provider == "rbb24" || has_any(&signals, GERMANY_TERMS) {
        categories.push(Category::Germany);
    }
    categories.extend(
        [
            (Category::Technology, has_any(&signals, TECHNOLOGY_TERMS)),
            (
                Category::GlobalEconomics,
                has_any(&signals, ECONOMICS_TERMS),
            ),
            (
                Category::GlobalPolitics,
                has_any(&signals, POLITICS_TERMS)
                    || joined.contains("foreign policy")
                    || joined.contains("international affairs"),
            ),
            (Category::Security, has_any(&signals, SECURITY_TERMS)),
            (Category::Climate, has_any(&signals, CLIMATE_TERMS)),
            (Category::Science, has_any(&signals, SCIENCE_TERMS)),
        ]
        .into_iter()
        .filter_map(|(category, qualifies)| qualifies.then_some(category)),
    );
    if categories.is_empty() {
        categories.push(Category::World);
    }
    categories
}

pub(crate) fn fit_summary(summary: &str, limit: usize) -> String {
    let sentences = leading_sentences(summary).collect::<Vec<_>>();
    let first = sentences.first().copied().unwrap_or(summary).trim();
    if word_count(first) > limit {
        return format!(
            "{}…",
            first
                .split_whitespace()
                .take(limit)
                .collect::<Vec<_>>()
                .join(" ")
                .trim_end_matches(|character: char| !character.is_alphanumeric())
        );
    }
    sentences
        .into_iter()
        .map(str::trim)
        .scan(0_usize, |words, sentence| {
            let next = (*words).saturating_add(word_count(sentence));
            if next > limit {
                None
            } else {
                *words = next;
                Some(sentence)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn leading_sentences(value: &str) -> impl Iterator<Item = &str> {
    value
        .split_inclusive(['.', '!', '?'])
        .filter(|sentence| !sentence.trim().is_empty())
}

fn story_id(headline: &str, canonical_url: &str) -> String {
    let slug = headline
        .split_whitespace()
        .filter_map(|word| {
            let normalized = word
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .flat_map(char::to_lowercase)
                .collect::<String>();
            (!normalized.is_empty()).then_some(normalized)
        })
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    let digest = Sha256::digest(canonical_url.as_bytes());
    let hash = digest
        .iter()
        .take(4)
        .fold(String::new(), |mut output, byte| {
            let _write_result = write!(output, "{byte:02x}");
            output
        });
    format!("{slug}-{hash}")
}

fn normalized_words(value: &str) -> HashSet<String> {
    let normalized = value.chars().fold(
        String::with_capacity(value.len()),
        |mut output, character| {
            if character.is_alphanumeric() {
                output.extend(character.to_lowercase());
            } else {
                output.push(' ');
            }
            output
        },
    );
    normalized.split_whitespace().map(Into::into).collect()
}

fn has_any(signals: &HashSet<String>, terms: &[&str]) -> bool {
    terms.iter().any(|term| signals.contains(*term))
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn title_and_summary(source_title: &str, value: &str) -> (String, String) {
    let (first_line, remaining) = value.split_once('\n').unwrap_or((value, ""));
    let first_line = first_line.trim();
    let (title, summary) = first_line
        .strip_prefix("Title: ")
        .map_or_else(|| (source_title, value), |title| (title, remaining));
    (strip_title_suffix(title), collapse_whitespace(summary))
}

fn strip_title_suffix(value: &str) -> String {
    collapse_whitespace(value.split_once('|').map_or(value, |(title, _)| title))
}

fn safe_image_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
    })
}

fn word_count(value: &str) -> usize {
    value.split_whitespace().count()
}

fn is_transient_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn retry_delay(response: &reqwest::Response, attempt: usize) -> Duration {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after)
        .map_or_else(
            || backoff(attempt),
            |duration| duration.min(MAX_RETRY_DELAY),
        )
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    value
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
        .or_else(|| {
            httpdate::parse_http_date(value)
                .ok()
                .and_then(|date| date.duration_since(SystemTime::now()).ok())
        })
}

fn backoff(attempt: usize) -> Duration {
    let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(4).min(4);
    Duration::from_secs(2_u64.saturating_pow(exponent))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExaError {
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    tag: Option<String>,
}

fn parse_error_response(status: StatusCode, body: Option<&str>) -> GeneratorError {
    let parsed = body.and_then(|value| serde_json::from_str::<ExaError>(value).ok());
    GeneratorError::ExaStatus {
        status: status.as_u16(),
        request_id: parsed.as_ref().and_then(|error| error.request_id.clone()),
        tag: parsed.as_ref().and_then(|error| error.tag.clone()),
        message: parsed
            .and_then(|error| error.error)
            .or_else(|| body.map(|value| value.chars().take(800).collect()))
            .unwrap_or_else(|| "response body unavailable".into()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use url::Url;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };

    use super::{
        EVENING_QUERY, ExaClient, ExaResponse, ExaResult, MORNING_QUERY, SUMMARY_PROMPT,
        build_stories, carried_story, carry_candidates, classify, edition_profile, fit_summary,
        merge_candidates, normalize, order_and_limit_candidates, parse_error_response,
        photo_candidates, retry_domains, search_request, select,
    };
    use crate::model::{Category, EditionName, EditionV1, LeadImageV1, ResearchEdition, StoryV1};
    use crate::schedule::{berlin_day, prior_day_window, publication_window};

    fn now() -> std::result::Result<chrono::DateTime<Utc>, Box<dyn std::error::Error>> {
        Utc.with_ymd_and_hms(2026, 8, 5, 10, 0, 0)
            .single()
            .ok_or_else(|| "invalid test time".into())
    }

    fn fixture() -> std::result::Result<ExaResponse, serde_json::Error> {
        serde_json::from_str(include_str!("../fixtures/exa-response.json"))
    }

    fn deep_fixture() -> std::result::Result<ExaResponse, serde_json::Error> {
        serde_json::from_str(include_str!("../fixtures/exa-deep-response.json"))
    }

    fn previous_edition(
        story_count: usize,
    ) -> std::result::Result<EditionV1, Box<dyn std::error::Error>> {
        let research = serde_json::from_str::<ResearchEdition>(include_str!(
            "../fixtures/valid-research.json"
        ))?;
        if research.stories.is_empty() {
            return Err("fixture has no stories".into());
        }
        let stories = (0..story_count)
            .filter_map(|index| {
                let template = research.stories.get(index % research.stories.len())?;
                let mut story = StoryV1::from(template);
                if index >= research.stories.len() {
                    story.id = format!("{}-carry-{index}", story.id);
                    if let Some(source) = story.sources.first_mut() {
                        source.url = format!("{}?carry={index}", source.url);
                    }
                }
                Some(story)
            })
            .collect::<Vec<_>>();
        let story_id = stories
            .first()
            .map_or_else(|| "unused-photo-story".into(), |story| story.id.clone());
        Ok(EditionV1 {
            schema_version: 1,
            edition_id: "previous-evening".into(),
            edition_name: EditionName::Evening,
            display_date: "Tuesday, 4 August 2026".into(),
            generated_at: "2026-08-04T16:00:00Z".parse()?,
            next_scheduled_at: "2026-08-05T06:00:00+02:00".parse()?,
            stories,
            lead_image: LeadImageV1 {
                story_id,
                url: "https://example.com/photo.jpg".into(),
                alt: "Previous photo".into(),
                credit: "Previous credit".into(),
                source_page_url: "https://www.reuters.com/world/previous/photo".into(),
            },
        })
    }

    fn novel_results(count: usize) -> Vec<serde_json::Value> {
        (0..count)
            .map(|index| {
                let url = if index % 2 == 0 {
                    format!("https://www.wsj.com/world/europe/novel-{index}")
                } else {
                    format!("https://www.nytimes.com/2026/08/05/world/novel-{index}")
                };
                json!({
                    "title": format!("Novel report {index}"),
                    "url": url,
                    "publishedDate": "2026-08-05T01:00:00Z",
                    "summary": format!("Novel report {index} contains enough safe copy for deterministic selection.")
                })
            })
            .collect()
    }

    fn request() -> std::result::Result<super::SearchRequest<'static>, Box<dyn std::error::Error>> {
        let profile = edition_profile(&EditionName::Morning);
        Ok(search_request(
            profile,
            publication_window(now()?, None)?,
            profile.domains,
        ))
    }

    fn normalized(
        results: &[super::ExaResult],
    ) -> std::result::Result<Vec<super::Candidate>, Box<dyn std::error::Error>> {
        let profile = edition_profile(&EditionName::Morning);
        Ok(normalize(
            results,
            publication_window(now()?, None)?,
            profile.domains,
            &HashSet::new(),
            0,
            true,
        )
        .candidates)
    }

    fn selected(
        response: &ExaResponse,
    ) -> std::result::Result<crate::model::ResearchEdition, Box<dyn std::error::Error>> {
        let candidates = normalized(&response.results)?;
        let (selected, _) = select(&candidates, &[]).ok_or("not enough candidates")?;
        let stories = build_stories(selected);
        Ok(ResearchEdition {
            photo_candidates: photo_candidates(&stories),
            stories,
        })
    }

    #[test]
    fn builds_exact_request_and_summer_boundaries()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let at = Utc
            .with_ymd_and_hms(2026, 8, 5, 4, 15, 0)
            .single()
            .ok_or("invalid morning time")?;
        let window = publication_window(at, None)?;
        let profile = edition_profile(&EditionName::Morning);
        let body = serde_json::to_value(search_request(profile, window, profile.domains))?;
        assert_eq!(body["query"], MORNING_QUERY);
        assert_eq!(body["category"], "news");
        assert_eq!(body["type"], "deep");
        assert_eq!(body["numResults"], 10);
        assert_eq!(
            body["includeDomains"],
            json!(["wsj.com", "nytimes.com", "ft.com", "reuters.com"])
        );
        assert!(
            body["systemPrompt"]
                .as_str()
                .is_some_and(|prompt| prompt.contains("Prefer 3 distinct sources when available"))
        );
        assert_eq!(body["contents"]["summary"]["query"], SUMMARY_PROMPT);
        assert_eq!(body["startPublishedDate"], "2026-08-04T15:00:00.000Z");
        assert_eq!(body["endPublishedDate"], "2026-08-05T04:44:59.999Z");
        assert_eq!(body["outputSchema"]["type"], "object");
        assert_eq!(
            body["outputSchema"]["properties"]["stories"]["items"]["required"],
            json!(["title", "url", "publishedDate", "summary"])
        );
        assert_eq!(body["stream"], false);
        assert_eq!(body["contents"]["maxAgeHours"], 0);

        let evening_at = Utc
            .with_ymd_and_hms(2026, 8, 5, 15, 0, 0)
            .single()
            .ok_or("invalid evening time")?;
        let evening_window = publication_window(evening_at, None)?;
        let evening_profile = edition_profile(&EditionName::Evening);
        let evening = serde_json::to_value(search_request(
            evening_profile,
            evening_window,
            evening_profile.domains,
        ))?;
        assert_eq!(evening["query"], EVENING_QUERY);
        assert_eq!(
            evening["includeDomains"],
            json!([
                "handelsblatt.com",
                "tagesschau.de",
                "ft.com",
                "dw.com",
                "bbc.com"
            ])
        );
        assert_eq!(evening["startPublishedDate"], "2026-08-04T22:00:00.000Z");
        assert_eq!(evening["endPublishedDate"], "2026-08-05T15:29:59.999Z");
        Ok(())
    }

    #[test]
    fn computes_winter_boundaries() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let winter = Utc
            .with_ymd_and_hms(2026, 12, 5, 10, 0, 0)
            .single()
            .ok_or("invalid winter time")?;
        let day = berlin_day(winter)?;
        assert_eq!(day.start.to_rfc3339(), "2026-12-04T23:00:00+00:00");
        assert_eq!(day.end.to_rfc3339(), "2026-12-05T23:00:00+00:00");
        Ok(())
    }

    #[test]
    fn fixture_parses_null_and_undocumented_fields()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let response = fixture()?;
        assert_eq!(response.results.len(), 10);
        assert_eq!(response.request_id.as_deref(), Some("exa-test-request"));
        assert!(
            response
                .results
                .first()
                .is_some_and(|result| result.image.is_some())
        );
        assert!(
            response
                .results
                .get(1)
                .is_some_and(|result| result.image.is_none())
        );
        Ok(())
    }

    #[test]
    fn deep_response_uses_only_grounded_structured_stories()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let response = deep_fixture()?;
        assert!(response.results.is_empty());

        let results = response.research_results();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results.first().map(|result| result.title.as_str()),
            Some("Allies open urgent regional security talks")
        );
        assert_eq!(
            results.first().and_then(|result| result.image.as_deref()),
            None
        );
        assert_eq!(normalized(&results)?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn enriches_selected_stories_with_exact_cached_image_metadata()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let mut edition = selected(&fixture()?)?;
        edition
            .stories
            .iter_mut()
            .for_each(|story| story.image_url = None);
        let source_url = edition
            .stories
            .first()
            .and_then(|story| story.sources.first())
            .map(|source| source.url.clone())
            .ok_or("selected story must have a source")?;
        Mock::given(method("POST"))
            .and(path("/contents"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [
                    {
                        "id": source_url,
                        "url": source_url,
                        "image": "https://static.reuters.com/images/resolved.jpg"
                    },
                    {
                        "url": "https://unrequested.example/story",
                        "image": "https://unrequested.example/image.jpg"
                    }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let base = Url::parse(&format!("{}/", server.uri()))?;
        let client = ExaClient::new(reqwest::Client::new(), &base, "test-key")?;
        let enriched = client.enrich_images(&mut edition).await?;

        assert_eq!(enriched, 1);
        assert_eq!(
            edition
                .stories
                .first()
                .and_then(|story| story.image_url.as_deref()),
            Some("https://static.reuters.com/images/resolved.jpg")
        );
        assert!(
            edition
                .stories
                .iter()
                .skip(1)
                .all(|story| story.image_url.is_none())
        );
        let requests = server
            .received_requests()
            .await
            .ok_or("request recording is disabled")?;
        let body = requests
            .first()
            .map(|request| serde_json::from_slice::<serde_json::Value>(&request.body))
            .transpose()?
            .ok_or("contents request must be recorded")?;
        assert_eq!(body["maxAgeHours"], -1);
        assert_eq!(body["urls"].as_array().map(Vec::len), Some(4));
        Ok(())
    }

    #[test]
    fn candidate_cap_preserves_primary_window_rank_order()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let profile = edition_profile(&EditionName::Morning);
        let primary = publication_window(now()?, None)?;
        let primary_results = (0..20)
            .map(|index| ExaResult {
                title: format!("Primary story {index}"),
                url: format!("https://www.nytimes.com/2026/08/05/world/story-{index}"),
                published_date: Some("2026-08-05T01:00:00Z".into()),
                image: None,
                summary: Some(format!(
                    "Primary report {index} contains enough safe copy for deterministic normalization."
                )),
            })
            .collect::<Vec<_>>();
        let mut candidates = normalize(
            &primary_results,
            primary,
            profile.domains,
            &HashSet::new(),
            0,
            true,
        )
        .candidates;
        let prior = prior_day_window(primary)?;
        let prior_results = vec![ExaResult {
            title: "Prior story".into(),
            url: "https://www.ft.com/content/prior-story".into(),
            published_date: Some("2026-08-04T12:00:00Z".into()),
            image: None,
            summary: Some("Prior report contains enough safe copy for normalization.".into()),
        }];
        merge_candidates(
            &mut candidates,
            normalize(
                &prior_results,
                prior,
                profile.domains,
                &HashSet::new(),
                2,
                false,
            )
            .candidates,
        );
        candidates.reverse();
        order_and_limit_candidates(&mut candidates);
        assert_eq!(candidates.len(), 18);
        assert!(candidates.iter().all(|candidate| candidate.primary_window));
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.rank)
                .collect::<Vec<_>>(),
            (0..18).map(|rank| (0, rank)).collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn filters_invalid_carries_and_preserves_deployed_order()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut previous = previous_edition(6)?;
        let expected = [
            previous
                .stories
                .get(3)
                .map(|story| story.id.clone())
                .ok_or("missing fourth story")?,
            previous
                .stories
                .get(5)
                .map(|story| story.id.clone())
                .ok_or("missing sixth story")?,
        ];
        if let Some(story) = previous.stories.get_mut(0) {
            story.id.clear();
        }
        if let Some(source) = previous
            .stories
            .get_mut(1)
            .and_then(|story| story.sources.first_mut())
        {
            source.name = "Wrong provider".into();
        }
        if let Some(story) = previous.stories.get_mut(2) {
            story.published_at = now()? + chrono::Duration::minutes(31);
        }
        let duplicate_source = previous
            .stories
            .get(3)
            .and_then(|story| story.sources.first())
            .cloned()
            .ok_or("missing duplicate source")?;
        if let Some(story) = previous.stories.get_mut(4) {
            story.sources = vec![duplicate_source];
        }

        assert!(carried_story(&previous.stories[0], now()?).is_none());
        let carried = carry_candidates(Some(&previous), now()?, &[]);
        assert_eq!(
            carried
                .iter()
                .map(|story| story.id.clone())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(carried.iter().all(|story| story.is_carried));
        Ok(())
    }

    #[tokio::test]
    async fn fills_two_novel_stories_with_four_deployed_stories()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "requestId": "two-novel",
                "results": novel_results(2)
            })))
            .expect(2)
            .mount(&server)
            .await;
        let previous = previous_edition(6)?;
        let expected_carry_ids = previous
            .stories
            .iter()
            .take(2)
            .map(|story| story.id.clone())
            .collect::<Vec<_>>();
        let base = Url::parse(&format!("{}/", server.uri()))?;
        let client = ExaClient::new(reqwest::Client::new(), &base, "test-key")?;
        let edition = client.research(now()?, Some(&previous)).await?;

        assert_eq!(edition.stories.len(), 4);
        assert!(
            edition
                .stories
                .iter()
                .take(2)
                .all(|story| !story.is_carried)
        );
        assert_eq!(
            edition
                .stories
                .iter()
                .skip(2)
                .map(|story| story.id.clone())
                .collect::<Vec<_>>(),
            expected_carry_ids
        );
        assert!(edition.stories.iter().skip(2).all(|story| story.is_carried));
        assert!(!edition.stories[0].is_carried);
        let requests = server
            .received_requests()
            .await
            .ok_or("request recording is disabled")?;
        assert_eq!(requests.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn reports_exact_combined_content_shortfall()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "requestId": "two-novel",
                "results": novel_results(2)
            })))
            .up_to_n_times(2)
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "requestId": "prior-empty",
                "results": []
            })))
            .expect(1)
            .mount(&server)
            .await;
        let previous = previous_edition(1)?;
        let base = Url::parse(&format!("{}/", server.uri()))?;
        let client = ExaClient::new(reqwest::Client::new(), &base, "test-key")?;
        let error = client
            .research(now()?, Some(&previous))
            .await
            .err()
            .ok_or("shortfall unexpectedly succeeded")?;
        let crate::error::GeneratorError::Validation(message) = error else {
            return Err("unexpected shortfall error variant".into());
        };
        assert_eq!(
            message,
            "only 2 novel and 1 carried stories were usable; four are required"
        );
        Ok(())
    }

    #[tokio::test]
    async fn refuses_to_publish_without_any_novel_lead()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "requestId": "empty",
                "results": []
            })))
            .expect(3)
            .mount(&server)
            .await;
        let base = Url::parse(&format!("{}/", server.uri()))?;
        let client = ExaClient::new(reqwest::Client::new(), &base, "test-key")?;
        let error = client
            .research(now()?, None)
            .await
            .err()
            .ok_or("empty research unexpectedly succeeded")?;
        let crate::error::GeneratorError::Validation(message) = error else {
            return Err("unexpected no-lead error variant".into());
        };
        assert_eq!(
            message,
            "exa returned no novel usable stories after the prior-day fallback; refusing to publish without a fresh lead"
        );
        Ok(())
    }

    #[test]
    fn filters_freshness_safety_and_duplicate_urls()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut results = fixture()?.results;
        let duplicate_url = results
            .first()
            .map(|result| result.url.clone())
            .ok_or("missing first result")?;
        let duplicate = results.last_mut().ok_or("missing last result")?;
        duplicate.url = duplicate_url;
        let candidates = normalized(&results)?;
        let window = publication_window(now()?, None)?;
        assert_eq!(candidates.len(), 7);
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.published_at >= window.start)
        );
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.provider == "Reuters")
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    fn leaves_same_event_detection_to_exa() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let candidates = normalized(&fixture()?.results)?;
        assert_eq!(candidates.len(), 8);
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.provider == "Reuters")
                .count(),
            2
        );
        Ok(())
    }

    #[test]
    fn counts_each_normalization_rejection_reason()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut results = fixture()?.results;
        let duplicate = results.get(4).cloned().ok_or("missing duplicate control")?;
        results.push(duplicate);
        if let Some(result) = results.get_mut(1) {
            result.summary = None;
        }
        if let Some(result) = results.get_mut(2) {
            result.published_date = None;
        }
        let previous =
            ["https://www.reuters.com/world/europe/atlantic-security-talks-2026-08-05".into()]
                .into_iter()
                .collect::<HashSet<_>>();
        let profile = edition_profile(&EditionName::Morning);
        let normalization = normalize(
            &results,
            publication_window(now()?, None)?,
            profile.domains,
            &previous,
            0,
            true,
        );
        assert_eq!(normalization.candidates.len(), 5);
        assert_eq!(normalization.rejections.missing_or_invalid_copy, 1);
        assert_eq!(normalization.rejections.missing_or_invalid_date, 1);
        assert_eq!(normalization.rejections.outside_request_window, 1);
        assert_eq!(normalization.rejections.disallowed_or_non_article_url, 1);
        assert_eq!(normalization.rejections.duplicate_canonical_url, 1);
        assert_eq!(normalization.rejections.repeated_previous_edition_url, 1);
        Ok(())
    }

    #[test]
    fn live_normalization_enforces_active_edition_profile()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let results = vec![
            ExaResult {
                title: "German policy report".into(),
                url: "https://www.dw.com/en/germany/policy/a-123456".into(),
                published_date: Some("2026-08-05T01:00:00Z".into()),
                image: None,
                summary: Some(
                    "German officials announced a consequential policy change today.".into(),
                ),
            },
            ExaResult {
                title: "International policy report".into(),
                url: "https://www.reuters.com/world/europe/policy-report".into(),
                published_date: Some("2026-08-05T01:00:00Z".into()),
                image: None,
                summary: Some(
                    "International officials announced a consequential policy change today.".into(),
                ),
            },
        ];
        let evening_at = Utc
            .with_ymd_and_hms(2026, 8, 5, 15, 0, 0)
            .single()
            .ok_or("invalid evening time")?;
        let evening_profile = edition_profile(&EditionName::Evening);
        let evening = normalize(
            &results,
            publication_window(evening_at, None)?,
            evening_profile.domains,
            &HashSet::new(),
            0,
            true,
        );
        assert_eq!(evening.candidates.len(), 1);
        assert_eq!(evening.candidates[0].provider, "DW");
        assert_eq!(evening.rejections.disallowed_or_non_article_url, 1);

        let morning_profile = edition_profile(&EditionName::Morning);
        let morning = normalize(
            &results,
            publication_window(now()?, None)?,
            morning_profile.domains,
            &HashSet::new(),
            0,
            true,
        );
        assert_eq!(morning.candidates.len(), 1);
        assert_eq!(morning.candidates[0].provider, "Reuters");
        assert_eq!(morning.rejections.disallowed_or_non_article_url, 1);
        Ok(())
    }

    #[test]
    fn rotates_stale_and_dominant_domains_without_dropping_below_two()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let profile = edition_profile(&EditionName::Morning);
        let results = fixture()?.results;
        let previous = [
            "https://www.reuters.com/world/europe/atlantic-security-talks-2026-08-05",
            "https://www.ft.com/content/00000000-0000-0000-0000-000000000001",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<HashSet<_>>();
        assert_eq!(
            retry_domains(profile.domains, profile.domains, &results, &previous),
            ["wsj.com", "nytimes.com"]
        );

        let no_previous = HashSet::new();
        assert_eq!(
            retry_domains(profile.domains, profile.domains, &results, &no_previous),
            ["wsj.com", "nytimes.com", "ft.com"]
        );
        assert_eq!(
            retry_domains(profile.domains, profile.domains, &[], &no_previous),
            ["nytimes.com", "ft.com", "reuters.com"]
        );
        assert_eq!(
            retry_domains(
                profile.domains,
                &["ft.com", "reuters.com"],
                &results,
                &previous
            ),
            ["ft.com", "reuters.com"]
        );
        Ok(())
    }

    #[test]
    fn uses_english_summary_title_and_strips_source_suffixes() {
        let summary = "Officials approved the extensive new plan for rail construction in Berlin and Brandenburg on Wednesday. Work starts Friday. Extra words follow here.";
        assert_eq!(
            fit_summary(summary, 18),
            "Officials approved the extensive new plan for rail construction in Berlin and Brandenburg on Wednesday. Work starts Friday."
        );
        assert!(fit_summary(summary, 8).ends_with('…'));
        assert_eq!(
            super::title_and_summary(
                "Deutscher Originaltitel | Reuters",
                "Title: English translated title | Reuters\nFirst sentence. Second sentence.",
            ),
            (
                "English translated title".into(),
                "First sentence. Second sentence.".into(),
            )
        );
        assert_eq!(
            super::title_and_summary(
                "Fallback title | tagesschau.de",
                "Summary without requested title line.",
            ),
            (
                "Fallback title".into(),
                "Summary without requested title line.".into(),
            )
        );
    }

    #[test]
    fn classifies_signals_in_priority_order() {
        let categories = classify(
            "https://www.rbb24.de/wirtschaft/beitrag/story.html",
            "KI-Chips für Berlin",
            "The German government backed technology investment and finance.",
            "rbb24",
        );
        assert_eq!(categories.first(), Some(&Category::Germany));
        assert!(categories.contains(&Category::Technology));
        assert!(categories.contains(&Category::GlobalEconomics));
    }

    #[test]
    fn selects_four_without_turning_preferences_into_blockers()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let response = fixture()?;
        let edition = selected(&response)?;
        assert_eq!(edition.stories.len(), 4);
        assert!(edition.stories.iter().all(|story| story.sources.len() == 1));
        assert!(edition.stories.iter().all(|story| !story.is_developing));
        assert!(
            edition
                .stories
                .iter()
                .all(|story| !story.headline.is_empty())
        );
        assert_eq!(edition.photo_candidates.len(), 4);
        assert_eq!(
            edition.photo_candidates.first(),
            edition
                .stories
                .iter()
                .find(|story| story.image_url.is_some())
                .map(|story| &story.id)
        );
        Ok(())
    }

    #[test]
    fn selects_all_novel_when_fewer_than_four_survive()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut response = fixture()?;
        response.results.truncate(3);
        assert_eq!(selected(&response)?.stories.len(), 3);
        Ok(())
    }

    #[test]
    fn selects_three_one_provider_split_across_novel_and_carried_stories()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let results = (0..5)
            .map(|index| ExaResult {
                title: format!("Reuters report {index}"),
                url: format!("https://www.reuters.com/world/europe/report-{index}"),
                published_date: Some("2026-08-05T01:00:00Z".into()),
                image: None,
                summary: Some(format!(
                    "Reuters report {index} contains enough safe copy for deterministic selection."
                )),
            })
            .collect::<Vec<_>>();
        let profile = edition_profile(&EditionName::Morning);
        let candidates = normalize(
            &results,
            publication_window(now()?, None)?,
            profile.domains,
            &HashSet::new(),
            0,
            true,
        )
        .candidates;
        let previous = previous_edition(6)?;
        let carried = carry_candidates(Some(&previous), now()?, &candidates);
        let (novel, selected_carries) =
            select(&candidates, &carried).ok_or("provider split was not selected")?;

        assert_eq!(novel.len(), 3);
        assert_eq!(selected_carries.len(), 1);
        assert!(super::provider_split_allowed(&novel, &selected_carries));
        assert!(
            selected_carries
                .iter()
                .all(|story| story.sources[0].name != "Reuters")
        );
        Ok(())
    }

    #[test]
    fn rejects_four_story_single_provider_selection()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let results = (0..4)
            .map(|index| ExaResult {
                title: format!("Reuters report {index}"),
                url: format!("https://www.reuters.com/world/europe/report-{index}"),
                published_date: Some("2026-08-05T01:00:00Z".into()),
                image: None,
                summary: Some(format!(
                    "Reuters report {index} contains enough safe copy for deterministic selection."
                )),
            })
            .collect::<Vec<_>>();
        let profile = edition_profile(&EditionName::Morning);
        let candidates = normalize(
            &results,
            publication_window(now()?, None)?,
            profile.domains,
            &HashSet::new(),
            0,
            true,
        )
        .candidates;

        assert!(select(&candidates, &[]).is_none());
        Ok(())
    }

    #[test]
    fn permanent_errors_include_request_id_and_tag() {
        let error = parse_error_response(
            reqwest::StatusCode::PAYMENT_REQUIRED,
            Some(r#"{"requestId":"req-402","error":"credits exhausted","tag":"NO_MORE_CREDITS"}"#),
        );
        let message = error.to_string();
        assert!(message.contains("req-402"));
        assert!(message.contains("NO_MORE_CREDITS"));
    }

    #[tokio::test]
    async fn sends_bearer_auth_and_retries_transient_statuses()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "requestId": "retried",
                "results": []
            })))
            .mount(&server)
            .await;
        let base = Url::parse(&format!("{}/", server.uri()))?;
        let client = ExaClient::new(reqwest::Client::new(), &base, "test-key")?;
        let body = request()?;
        let response = client
            .send_with_retry::<ExaResponse>(&client.search_endpoint, &body)
            .await?;
        assert_eq!(response.request_id.as_deref(), Some("retried"));
        let requests = server
            .received_requests()
            .await
            .ok_or("request recording is disabled")?;
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests.first().map(|request| request.body.as_slice()),
            requests.get(1).map(|request| request.body.as_slice())
        );
        Ok(())
    }

    #[tokio::test]
    async fn reruns_and_combines_search_when_too_few_stories_survive()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let mut initial = serde_json::from_str::<serde_json::Value>(include_str!(
            "../fixtures/exa-response.json"
        ))?;
        initial["requestId"] = json!("initial-search");
        let initial_results = initial["results"]
            .as_array_mut()
            .ok_or("fixture results must be an array")?;
        initial_results.truncate(3);
        let mut fallback = serde_json::from_str::<serde_json::Value>(include_str!(
            "../fixtures/exa-response.json"
        ))?;
        fallback["requestId"] = json!("fallback-search");

        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(initial))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fallback))
            .expect(1)
            .mount(&server)
            .await;

        let base = Url::parse(&format!("{}/", server.uri()))?;
        let client = ExaClient::new(reqwest::Client::new(), &base, "test-key")?;
        let edition = client.research(now()?, None).await?;
        let requests = server
            .received_requests()
            .await
            .ok_or("request recording is disabled")?;
        assert_eq!(requests.len(), 2);
        let bodies = requests
            .iter()
            .map(|request| serde_json::from_slice::<serde_json::Value>(&request.body))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(
            bodies.first().map(|body| &body["includeDomains"]),
            Some(&json!(["wsj.com", "nytimes.com", "ft.com", "reuters.com"]))
        );
        assert_eq!(
            bodies.get(1).map(|body| &body["includeDomains"]),
            Some(&json!(["nytimes.com", "ft.com", "reuters.com"]))
        );
        assert!(
            bodies
                .iter()
                .all(|body| body["contents"]["maxAgeHours"] == 0)
        );
        assert_eq!(edition.stories.len(), 4);
        Ok(())
    }

    #[tokio::test]
    async fn does_not_rerun_search_when_enough_stories_survive()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let response = serde_json::from_str::<serde_json::Value>(include_str!(
            "../fixtures/exa-response.json"
        ))?;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;

        let base = Url::parse(&format!("{}/", server.uri()))?;
        let client = ExaClient::new(reqwest::Client::new(), &base, "test-key")?;
        let edition = client.research(now()?, None).await?;
        let requests = server
            .received_requests()
            .await
            .ok_or("request recording is disabled")?;
        assert_eq!(requests.len(), 1);
        assert_eq!(edition.stories.len(), 4);
        Ok(())
    }

    #[tokio::test]
    async fn uses_adjacent_prior_day_window_after_combined_shortfall()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        for request_id in ["initial-empty", "retry-empty"] {
            Mock::given(method("POST"))
                .and(path("/search"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "requestId": request_id,
                    "costDollars": { "total": 0.01 },
                    "results": []
                })))
                .up_to_n_times(1)
                .expect(1)
                .mount(&server)
                .await;
        }
        let prior_results = (0..6)
            .map(|index| {
                let url = if index % 2 == 0 {
                    format!("https://www.ft.com/content/prior-{index}")
                } else {
                    format!("https://www.reuters.com/world/europe/prior-{index}")
                };
                json!({
                    "title": format!("Prior report {index}"),
                    "url": url,
                    "publishedDate": "2026-08-04T12:00:00Z",
                    "summary": format!("Prior report {index} contains enough safe copy for selection.")
                })
            })
            .collect::<Vec<_>>();
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "requestId": "prior-day",
                "costDollars": { "total": 0.02 },
                "results": prior_results
            })))
            .expect(1)
            .mount(&server)
            .await;

        let base = Url::parse(&format!("{}/", server.uri()))?;
        let client = ExaClient::new(reqwest::Client::new(), &base, "test-key")?;
        let edition = client.research(now()?, None).await?;
        assert_eq!(edition.stories.len(), 4);
        let requests = server
            .received_requests()
            .await
            .ok_or("request recording is disabled")?;
        assert_eq!(requests.len(), 3);
        let bodies = requests
            .iter()
            .map(|request| serde_json::from_slice::<serde_json::Value>(&request.body))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(
            bodies.first().map(|body| &body["startPublishedDate"]),
            Some(&json!("2026-08-04T15:00:00.000Z"))
        );
        assert_eq!(
            bodies.get(1).map(|body| &body["startPublishedDate"]),
            bodies.first().map(|body| &body["startPublishedDate"])
        );
        assert_eq!(
            bodies.get(2).map(|body| &body["startPublishedDate"]),
            Some(&json!("2026-08-03T22:00:00.000Z"))
        );
        assert_eq!(
            bodies.get(2).map(|body| &body["endPublishedDate"]),
            Some(&json!("2026-08-04T14:59:59.999Z"))
        );
        assert_eq!(
            bodies.get(2).map(|body| &body["includeDomains"]),
            Some(&json!(["ft.com", "reuters.com"]))
        );
        assert!(
            bodies
                .get(2)
                .and_then(|body| body["systemPrompt"].as_str())
                .is_some_and(|prompt| prompt.contains("Prefer 2 distinct sources when available"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn retries_server_errors() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(503).insert_header("Retry-After", "0"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "requestId": "server-retried",
                "results": []
            })))
            .mount(&server)
            .await;
        let base = Url::parse(&format!("{}/", server.uri()))?;
        let client = ExaClient::new(reqwest::Client::new(), &base, "test-key")?;
        let body = request()?;
        let response = client
            .send_with_retry::<ExaResponse>(&client.search_endpoint, &body)
            .await?;
        assert_eq!(response.request_id.as_deref(), Some("server-retried"));
        Ok(())
    }

    #[tokio::test]
    async fn does_not_retry_budget_errors() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(402).set_body_json(json!({
                "requestId": "budget-request",
                "error": "credits exhausted",
                "tag": "NO_MORE_CREDITS"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let base = Url::parse(&format!("{}/", server.uri()))?;
        let client = ExaClient::new(reqwest::Client::new(), &base, "test-key")?;
        let body = request()?;
        let Some(error) = client
            .send_with_retry::<ExaResponse>(&client.search_endpoint, &body)
            .await
            .err()
        else {
            return Err("budget error unexpectedly succeeded".into());
        };
        let message = error.to_string();
        assert!(message.contains("budget-request"));
        assert!(message.contains("NO_MORE_CREDITS"));
        Ok(())
    }

    #[test]
    fn earlier_ranks_break_equal_scores() {
        let first = super::SelectionScore {
            primary_window: 6,
            required_categories: 1,
            providers: 1,
            has_image: false,
            ranks: vec![(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (0, 5)],
        };
        let second = super::SelectionScore {
            ranks: vec![(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (1, 0)],
            ..first.clone()
        };
        assert!(first.better_than(&second));
        assert!(!second.better_than(&first));
    }

    #[test]
    fn selection_preferences_are_lexicographic() {
        let baseline = super::SelectionScore {
            primary_window: 6,
            required_categories: 1,
            providers: 1,
            has_image: false,
            ranks: vec![(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (0, 5)],
        };
        let category = super::SelectionScore {
            primary_window: 6,
            required_categories: 2,
            providers: 0,
            has_image: false,
            ranks: vec![(0, 4), (0, 5), (0, 6), (0, 7), (0, 8), (0, 9)],
        };
        let provider = super::SelectionScore {
            providers: 2,
            ..baseline.clone()
        };
        let image = super::SelectionScore {
            has_image: true,
            ..baseline.clone()
        };
        let prior_day = super::SelectionScore {
            primary_window: 5,
            required_categories: 4,
            providers: 3,
            has_image: true,
            ranks: vec![(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (2, 0)],
        };
        assert!(category.better_than(&baseline));
        assert!(provider.better_than(&baseline));
        assert!(image.better_than(&baseline));
        assert!(baseline.better_than(&prior_day));
        assert!(provider.better_than(&image));
    }
}
