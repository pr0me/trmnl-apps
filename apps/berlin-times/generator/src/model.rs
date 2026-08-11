use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Climate,
    Germany,
    GlobalEconomics,
    GlobalPolitics,
    Science,
    Security,
    Technology,
    World,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResearchSource {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResearchStory {
    pub id: String,
    pub primary_category: Category,
    pub is_developing: bool,
    pub headline: String,
    pub summary: String,
    pub published_at: DateTime<Utc>,
    pub sources: Vec<ResearchSource>,
    #[serde(default, skip_serializing)]
    pub image_url: Option<String>,
    #[serde(default, skip_serializing)]
    pub is_carried: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResearchEdition {
    pub stories: Vec<ResearchStory>,
    pub photo_candidates: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EditionName {
    Evening,
    Morning,
}

impl EditionName {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Evening => "evening",
            Self::Morning => "morning",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublishedSource {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoryV1 {
    pub id: String,
    pub primary_category: Category,
    pub is_developing: bool,
    pub headline: String,
    pub summary: String,
    pub published_at: DateTime<Utc>,
    pub sources: Vec<PublishedSource>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LeadImageV1 {
    pub story_id: String,
    pub url: String,
    pub alt: String,
    pub credit: String,
    pub source_page_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EditionV1 {
    pub schema_version: u8,
    pub edition_id: String,
    pub edition_name: EditionName,
    pub display_date: String,
    pub generated_at: DateTime<Utc>,
    pub next_scheduled_at: DateTime<FixedOffset>,
    pub stories: Vec<StoryV1>,
    pub lead_image: LeadImageV1,
}

impl From<&ResearchStory> for StoryV1 {
    fn from(story: &ResearchStory) -> Self {
        Self {
            id: story.id.clone(),
            primary_category: story.primary_category.clone(),
            is_developing: story.is_developing,
            headline: story.headline.clone(),
            summary: story.summary.clone(),
            published_at: story.published_at,
            sources: story
                .sources
                .iter()
                .map(|source| PublishedSource {
                    name: source.name.clone(),
                    url: source.url.clone(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ResearchEdition, StoryV1};

    #[test]
    fn carry_provenance_is_internal_only() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let research = serde_json::from_str::<ResearchEdition>(include_str!(
            "../fixtures/valid-research.json"
        ))?;
        assert!(research.stories.iter().all(|story| !story.is_carried));
        let serialized_research = serde_json::to_string(&research)?;
        assert!(!serialized_research.contains("is_carried"));

        let published = research
            .stories
            .first()
            .map(StoryV1::from)
            .ok_or("fixture has no stories")?;
        assert!(!serde_json::to_string(&published)?.contains("is_carried"));
        Ok(())
    }
}
