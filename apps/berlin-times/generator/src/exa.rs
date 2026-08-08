use std::{
    cmp::Ordering,
    collections::HashSet,
    fmt::Write as _,
    time::{Duration, SystemTime},
};

use chrono::{DateTime, SecondsFormat, Utc};
use reqwest::{StatusCode, header::RETRY_AFTER};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::time::sleep;
use tracing::{info, warn};
use url::Url;

use crate::{
    error::{GeneratorError, Result},
    model::{Category, EditionV1, ResearchEdition, ResearchSource, ResearchStory},
    schedule::berlin_day,
    validate::{article_url_allowed, canonicalize_url, provider_name},
};

const MAX_ATTEMPTS: usize = 3;
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
const REQUIRED_STORIES: usize = 6;
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

pub const SEARCH_QUERY: &str = "Today’s most important news in global politics OR global economics OR Berlin and Germany OR consequential technology";
pub const SYSTEM_PROMPT: &str = "Provide results from at least 3 sources (2 international and 1 German/Berlin one). Make sure to not emit news items that are duplicates between agencies.\nPrefer current reporting over analysis or opinion";
pub const SUMMARY_PROMPT: &str = "As your first line, output `Title: {english_title}\\n`, providing translations for German titles.\nSummarize the news article in 2 short English sentences / 30-45 words. Make the first sentence self-contained and no longer than 30 words.\nDeliver the gist. Write the summary as it would appear in a newspaper itself; do not use \"Summary:\", \"The article explains\" or alike.";
pub const DOMAINS: &[&str] = &[
    "nytimes.com",
    "ft.com",
    "tagesschau.de",
    "reuters.com",
    "rbb24.de",
    "wsj.com",
    "handelsblatt.com",
];

#[derive(Clone)]
pub struct ExaClient {
    http: reqwest::Client,
    endpoint: Url,
    api_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExaResponse {
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub search_type: Option<String>,
    #[serde(default)]
    pub cost_dollars: Option<CostDollars>,
    #[serde(default)]
    pub results: Vec<ExaResult>,
}

#[derive(Debug, Deserialize)]
pub struct CostDollars {
    #[serde(default)]
    pub total: Option<f64>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchRequest<'a> {
    query: &'a str,
    category: &'a str,
    #[serde(rename = "type")]
    search_type: &'a str,
    num_results: usize,
    include_domains: &'a [&'a str],
    system_prompt: &'a str,
    start_published_date: String,
    end_published_date: String,
    output_schema: OutputSchema<'a>,
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
struct Summary<'a> {
    query: &'a str,
}

#[derive(Debug, Serialize)]
struct OutputSchema<'a> {
    #[serde(rename = "type")]
    schema_type: &'a str,
}

#[derive(Debug)]
struct Candidate {
    rank: usize,
    canonical_url: String,
    provider: &'static str,
    title: String,
    summary: String,
    published_at: DateTime<Utc>,
    image_url: Option<String>,
    categories: Vec<Category>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectionScore {
    required_categories: usize,
    providers: usize,
    has_image: bool,
    novel_urls: usize,
    ranks: Vec<usize>,
}

impl ExaClient {
    pub fn new(http: reqwest::Client, api_base: &Url, api_key: impl Into<String>) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(GeneratorError::Config(
                "exa_api_key must not be empty".into(),
            ));
        }
        let endpoint = api_base
            .join("search")
            .map_err(|error| GeneratorError::Config(format!("invalid api base url: {error}")))?;
        Ok(Self {
            http,
            endpoint,
            api_key,
        })
    }

    pub async fn search(&self, now: DateTime<Utc>) -> Result<ExaResponse> {
        let body = search_request(now)?;
        self.send_with_retry(&body).await
    }

    pub async fn search_with_fallback(&self, now: DateTime<Utc>) -> Result<ExaResponse> {
        let mut response = self.search(now).await?;
        let usable = normalize(&response.results, now)?.len();
        if usable >= REQUIRED_STORIES {
            return Ok(response);
        }

        let initial_request_id = response.request_id.as_deref().unwrap_or("unavailable");
        warn!(
            request_id = initial_request_id,
            usable, "Exa search returned too few usable stories; running fallback"
        );
        let fallback = self.search(now).await?;
        let fallback_request_id = fallback.request_id.as_deref().unwrap_or("unavailable");
        let fallback_returned = fallback.results.len();
        response.cost_dollars = combined_cost(response.cost_dollars, fallback.cost_dollars);
        response.results.extend(fallback.results);
        info!(
            initial_request_id,
            fallback_request_id,
            fallback_returned,
            combined_returned = response.results.len(),
            "Exa fallback search completed"
        );
        Ok(response)
    }

    async fn send_with_retry(&self, body: &SearchRequest<'_>) -> Result<ExaResponse> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let response = self
                .http
                .post(self.endpoint.clone())
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

fn search_request(now: DateTime<Utc>) -> Result<SearchRequest<'static>> {
    let day = berlin_day(now)?;
    Ok(SearchRequest {
        query: SEARCH_QUERY,
        category: "news",
        search_type: "deep-reasoning",
        num_results: 10,
        include_domains: DOMAINS,
        system_prompt: SYSTEM_PROMPT,
        start_published_date: day.start.to_rfc3339_opts(SecondsFormat::Millis, true),
        end_published_date: day
            .end
            .checked_sub_signed(chrono::Duration::milliseconds(1))
            .unwrap_or(day.end)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
        output_schema: OutputSchema {
            schema_type: "object",
        },
        stream: false,
        contents: Contents {
            summary: Summary {
                query: SUMMARY_PROMPT,
            },
            max_age_hours: 0,
        },
    })
}

pub fn normalize_and_select(
    response: &ExaResponse,
    now: DateTime<Utc>,
    previous: Option<&EditionV1>,
) -> Result<ResearchEdition> {
    let returned = response.results.len();
    let request_id = response.request_id.as_deref().unwrap_or("unavailable");
    let search_type = response.search_type.as_deref().unwrap_or("unavailable");
    let cost = response.cost_dollars.as_ref().and_then(|value| value.total);
    let candidates = normalize(&response.results, now)?;
    let usable = candidates.len();
    if usable < REQUIRED_STORIES {
        return Err(GeneratorError::Validation(format!(
            "exa returned only {usable} fresh, safe stories; six are required"
        )));
    }
    let previous_urls = previous_urls(previous);
    let selected = select(&candidates, &previous_urls)
        .ok_or_else(|| GeneratorError::Validation("could not select six exa stories".into()))?;
    let edition = build_edition(selected);
    let mut providers = edition
        .stories
        .iter()
        .filter_map(|story| story.sources.first().map(|source| source.name.as_str()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    providers.sort_unstable();
    let mut categories = edition
        .stories
        .iter()
        .map(|story| format!("{:?}", story.primary_category))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    categories.sort_unstable();
    info!(
        request_id,
        search_type,
        returned,
        usable,
        selected = edition.stories.len(),
        provider_coverage = %providers.join(","),
        category_coverage = %categories.join(","),
        cost_dollars = cost,
        "Exa search normalized"
    );
    Ok(edition)
}

fn normalize(results: &[ExaResult], now: DateTime<Utc>) -> Result<Vec<Candidate>> {
    let day = berlin_day(now)?;
    let latest = now
        .checked_add_signed(chrono::Duration::minutes(30))
        .unwrap_or(now);
    let mut urls = HashSet::new();
    let mut candidates = Vec::<Candidate>::new();

    for (rank, result) in results.iter().enumerate() {
        let Some(raw_summary) = result.summary.as_deref() else {
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
            continue;
        }
        let Some(published_at) = result
            .published_date
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
        else {
            continue;
        };
        if published_at < day.start || published_at >= day.end || published_at > latest {
            continue;
        }
        if !article_url_allowed(&result.url) {
            continue;
        }
        let canonical_url = match canonicalize_url(&result.url) {
            Ok(value) if urls.insert(value.clone()) => value,
            Ok(_) | Err(_) => continue,
        };
        let Some(provider) = provider_name(&canonical_url) else {
            continue;
        };
        let categories = classify(&canonical_url, &title, &summary, provider);
        let image_url = result
            .image
            .as_ref()
            .filter(|value| safe_image_url(value))
            .cloned();
        candidates.push(Candidate {
            rank,
            canonical_url,
            provider,
            title,
            summary,
            published_at,
            image_url,
            categories,
        });
    }
    Ok(candidates)
}

fn combined_cost(
    initial: Option<CostDollars>,
    fallback: Option<CostDollars>,
) -> Option<CostDollars> {
    match (
        initial.and_then(|cost| cost.total),
        fallback.and_then(|cost| cost.total),
    ) {
        (Some(initial), Some(fallback)) => Some(CostDollars {
            total: Some(initial + fallback),
        }),
        (Some(total), None) | (None, Some(total)) => Some(CostDollars { total: Some(total) }),
        (None, None) => None,
    }
}

fn select<'a>(
    candidates: &'a [Candidate],
    previous_urls: &HashSet<String>,
) -> Option<Vec<&'a Candidate>> {
    let mut best = None::<(SelectionScore, Vec<&Candidate>)>;
    let mut current = Vec::with_capacity(REQUIRED_STORIES);
    enumerate_subsets(candidates, previous_urls, 0, &mut current, &mut best);
    best.map(|(_, candidates)| candidates)
}

fn enumerate_subsets<'a>(
    candidates: &'a [Candidate],
    previous_urls: &HashSet<String>,
    start: usize,
    current: &mut Vec<&'a Candidate>,
    best: &mut Option<(SelectionScore, Vec<&'a Candidate>)>,
) {
    if current.len() == REQUIRED_STORIES {
        let score = selection_score(current, previous_urls);
        if best
            .as_ref()
            .is_none_or(|(best_score, _)| score.better_than(best_score))
        {
            *best = Some((score, current.clone()));
        }
        return;
    }
    let needed = REQUIRED_STORIES.saturating_sub(current.len());
    if candidates.len().saturating_sub(start) < needed {
        return;
    }
    (start..candidates.len()).for_each(|index| {
        current.push(&candidates[index]);
        enumerate_subsets(candidates, previous_urls, index + 1, current, best);
        let _removed = current.pop();
    });
}

fn selection_score(candidates: &[&Candidate], previous_urls: &HashSet<String>) -> SelectionScore {
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
        required_categories,
        providers,
        has_image: candidates
            .iter()
            .any(|candidate| candidate.image_url.is_some()),
        novel_urls: candidates
            .iter()
            .filter(|candidate| !previous_urls.contains(&candidate.canonical_url))
            .count(),
        ranks: candidates.iter().map(|candidate| candidate.rank).collect(),
    }
}

impl SelectionScore {
    fn better_than(&self, other: &Self) -> bool {
        self.required_categories
            .cmp(&other.required_categories)
            .then_with(|| self.providers.cmp(&other.providers))
            .then_with(|| self.has_image.cmp(&other.has_image))
            .then_with(|| self.novel_urls.cmp(&other.novel_urls))
            .then_with(|| other.ranks.cmp(&self.ranks))
            == Ordering::Greater
    }
}

fn build_edition(selected: Vec<&Candidate>) -> ResearchEdition {
    let stories = selected
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
        })
        .collect::<Vec<_>>();
    let photo_candidates = stories
        .iter()
        .filter(|story| story.image_url.is_some())
        .chain(stories.iter().filter(|story| story.image_url.is_none()))
        .map(|story| story.id.clone())
        .collect();
    ResearchEdition {
        stories,
        photo_candidates,
    }
}

fn previous_urls(previous: Option<&EditionV1>) -> HashSet<String> {
    previous
        .into_iter()
        .flat_map(|edition| &edition.stories)
        .flat_map(|story| &story.sources)
        .filter_map(|source| canonicalize_url(&source.url).ok())
        .collect()
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
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use url::Url;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };

    use super::{
        DOMAINS, ExaClient, ExaResponse, SEARCH_QUERY, SUMMARY_PROMPT, SYSTEM_PROMPT, classify,
        fit_summary, normalize, normalize_and_select, parse_error_response, search_request,
    };
    use crate::model::Category;
    use crate::schedule::berlin_day;

    fn now() -> std::result::Result<chrono::DateTime<Utc>, Box<dyn std::error::Error>> {
        Utc.with_ymd_and_hms(2026, 8, 5, 10, 0, 0)
            .single()
            .ok_or_else(|| "invalid test time".into())
    }

    fn fixture() -> std::result::Result<ExaResponse, serde_json::Error> {
        serde_json::from_str(include_str!("../fixtures/exa-response.json"))
    }

    #[test]
    fn builds_exact_request_and_summer_boundaries()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let body = serde_json::to_value(search_request(now()?)?)?;
        assert_eq!(body["query"], SEARCH_QUERY);
        assert_eq!(body["category"], "news");
        assert_eq!(body["type"], "deep-reasoning");
        assert_eq!(body["numResults"], 10);
        assert_eq!(body["includeDomains"], json!(DOMAINS));
        assert_eq!(body["systemPrompt"], SYSTEM_PROMPT);
        assert_eq!(body["contents"]["summary"]["query"], SUMMARY_PROMPT);
        assert_eq!(body["startPublishedDate"], "2026-08-04T22:00:00.000Z");
        assert_eq!(body["endPublishedDate"], "2026-08-05T21:59:59.999Z");
        assert_eq!(body["outputSchema"], json!({ "type": "object" }));
        assert_eq!(body["stream"], false);
        assert_eq!(body["contents"]["maxAgeHours"], 0);
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
    fn filters_freshness_safety_and_duplicate_urls()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut results = fixture()?.results;
        let duplicate_url = results
            .first()
            .map(|result| result.url.clone())
            .ok_or("missing first result")?;
        let duplicate = results.last_mut().ok_or("missing last result")?;
        duplicate.url = duplicate_url;
        let candidates = normalize(&results, now()?)?;
        let day = berlin_day(now()?)?;
        assert_eq!(candidates.len(), 7);
        assert!(
            candidates
                .iter()
                .all(|candidate| { candidate.published_at >= day.start })
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
        let candidates = normalize(&fixture()?.results, now()?)?;
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
    fn selects_six_without_turning_preferences_into_blockers()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let response = fixture()?;
        let edition = normalize_and_select(&response, now()?, None)?;
        assert_eq!(edition.stories.len(), 6);
        assert!(edition.stories.iter().all(|story| story.sources.len() == 1));
        assert!(edition.stories.iter().all(|story| !story.is_developing));
        assert!(
            edition
                .stories
                .iter()
                .all(|story| !story.headline.is_empty())
        );
        assert_eq!(edition.photo_candidates.len(), 6);
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
    fn fails_when_fewer_than_six_survive() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut response = fixture()?;
        response.results.truncate(5);
        assert!(normalize_and_select(&response, now()?, None).is_err());
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
        let response = client.search(now()?).await?;
        assert_eq!(response.request_id.as_deref(), Some("retried"));
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
        initial_results.truncate(5);
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
        let response = client.search_with_fallback(now()?).await?;
        let requests = server
            .received_requests()
            .await
            .ok_or("request recording is disabled")?;
        assert_eq!(requests.len(), 2);
        let bodies = requests
            .iter()
            .map(|request| serde_json::from_slice::<serde_json::Value>(&request.body))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(bodies.first(), bodies.get(1));
        assert!(
            bodies
                .iter()
                .all(|body| body["contents"]["maxAgeHours"] == 0)
        );
        assert_eq!(normalize(&response.results, now()?)?.len(), 8);
        assert_eq!(
            response.cost_dollars.as_ref().and_then(|cost| cost.total),
            Some(0.034)
        );
        assert_eq!(
            normalize_and_select(&response, now()?, None)?.stories.len(),
            6
        );
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
        let response = client.search_with_fallback(now()?).await?;
        let requests = server
            .received_requests()
            .await
            .ok_or("request recording is disabled")?;
        assert_eq!(requests.len(), 1);
        assert_eq!(response.results.len(), 10);
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
        let response = client.search(now()?).await?;
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
        let Some(error) = client.search(now()?).await.err() else {
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
            required_categories: 1,
            providers: 1,
            has_image: false,
            novel_urls: 1,
            ranks: vec![0, 1, 2, 3, 4, 5],
        };
        let second = super::SelectionScore {
            ranks: vec![0, 1, 2, 3, 4, 6],
            ..first.clone()
        };
        assert!(first.better_than(&second));
        assert!(!second.better_than(&first));
    }

    #[test]
    fn selection_preferences_are_lexicographic() {
        let baseline = super::SelectionScore {
            required_categories: 1,
            providers: 1,
            has_image: false,
            novel_urls: 1,
            ranks: vec![0, 1, 2, 3, 4, 5],
        };
        let category = super::SelectionScore {
            required_categories: 2,
            providers: 0,
            has_image: false,
            novel_urls: 0,
            ranks: vec![4, 5, 6, 7, 8, 9],
        };
        let provider = super::SelectionScore {
            providers: 2,
            ..baseline.clone()
        };
        let image = super::SelectionScore {
            has_image: true,
            ..baseline.clone()
        };
        let novelty = super::SelectionScore {
            novel_urls: 2,
            ..baseline.clone()
        };
        assert!(category.better_than(&baseline));
        assert!(provider.better_than(&baseline));
        assert!(image.better_than(&baseline));
        assert!(novelty.better_than(&baseline));
        assert!(provider.better_than(&image));
        assert!(image.better_than(&novelty));
    }
}
