use std::{fmt::Write as _, fs, path::Path};

use chrono::{DateTime, Utc};
use html_escape::encode_text;
use sha2::{Digest, Sha256};
use tempfile::Builder;
use url::Url;

use crate::{
    error::{GeneratorError, Result, io_error},
    model::{EditionV1, LeadImageV1, ResearchEdition, StoryV1},
    photo::PhotoAsset,
    schedule,
};

const UNIFRAKTUR_COOK: &[u8] = include_bytes!("../assets/UnifrakturCook-Bold.ttf");
const SOURCE_SERIF: &[u8] = include_bytes!("../assets/SourceSerif4-Variable.ttf");
const FONT_LICENSE: &[u8] = include_bytes!("../assets/OFL.txt");
const PLUGIN_STYLES: &[u8] = include_bytes!("../assets/berlin-times.css");

pub fn assemble_edition(
    research: &ResearchEdition,
    photo: &PhotoAsset,
    public_base_url: &Url,
    now: DateTime<Utc>,
) -> Result<(EditionV1, String)> {
    let edition_id = edition_id(research, now)?;
    let photo_name = format!("lead-{edition_id}.jpg");
    let photo_url = directory_url(public_base_url)?
        .join(&format!("assets/{photo_name}"))
        .map_err(|error| GeneratorError::Config(format!("invalid public photo url: {error}")))?;
    let edition = EditionV1 {
        schema_version: 1,
        edition_id,
        edition_name: schedule::edition_name(now),
        display_date: schedule::display_date(now),
        generated_at: now,
        next_scheduled_at: schedule::next_scheduled_at(now)?,
        stories: research.stories.iter().map(StoryV1::from).collect(),
        lead_image: LeadImageV1 {
            story_id: photo.story_id.clone(),
            url: photo_url.into(),
            alt: photo.alt.clone(),
            credit: photo.credit.clone(),
            source_page_url: photo.source_page_url.clone(),
        },
    };
    Ok((edition, photo_name))
}

pub fn publish_site(
    output: impl AsRef<Path>,
    edition: &EditionV1,
    photo_name: &str,
    photo_bytes: &[u8],
) -> Result<()> {
    let output = output.as_ref();
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(io_error(parent))?;
    let temporary = Builder::new()
        .prefix(".berlin-times-")
        .tempdir_in(parent)
        .map_err(io_error(parent))?;
    let staging = temporary.path().join("site");
    let assets = staging.join("assets");
    let fonts = assets.join("fonts");
    fs::create_dir_all(&fonts).map_err(io_error(&fonts))?;

    let mut edition_json = serde_json::to_string_pretty(edition)?;
    edition_json.push('\n');
    write(staging.join("edition.json"), edition_json.as_bytes())?;
    write(assets.join(photo_name), photo_bytes)?;
    write(fonts.join("UnifrakturCook-Bold.ttf"), UNIFRAKTUR_COOK)?;
    write(fonts.join("SourceSerif4-Variable.ttf"), SOURCE_SERIF)?;
    write(fonts.join("OFL.txt"), FONT_LICENSE)?;
    write(assets.join("berlin-times.css"), PLUGIN_STYLES)?;
    write(staging.join("index.html"), status_page(edition).as_bytes())?;
    write(staging.join("robots.txt"), b"User-agent: *\nDisallow: /\n")?;
    write(staging.join(".nojekyll"), b"")?;

    replace_directory(output, &staging, temporary.path())
}

fn edition_id(research: &ResearchEdition, now: DateTime<Utc>) -> Result<String> {
    let serialized = serde_json::to_vec(&research.stories)?;
    let digest = Sha256::digest(serialized);
    let short_hash = digest
        .iter()
        .take(6)
        .fold(String::with_capacity(12), |mut output, byte| {
            let _write_result = write!(output, "{byte:02x}");
            output
        });
    Ok(format!("{}-{short_hash}", now.format("%Y%m%dT%H%M%SZ")))
}

fn directory_url(value: &Url) -> Result<Url> {
    let mut value = value.clone();
    if !value.path().ends_with('/') {
        let path = format!("{}/", value.path());
        value.set_path(&path);
    }
    if value.scheme() != "https" {
        return Err(GeneratorError::Config(
            "public base url must use https".into(),
        ));
    }
    Ok(value)
}

fn status_page(edition: &EditionV1) -> String {
    let stories = edition.stories.iter().enumerate().fold(
        String::new(),
        |mut output, (index, story)| {
            let links = story.sources.iter().fold(String::new(), |mut links, source| {
                if !links.is_empty() {
                    links.push_str(" · ");
                }
                let _write_result = write!(
                    links,
                    "<a href=\"{}\" rel=\"noreferrer\">{}</a>",
                    encode_text(&source.url),
                    encode_text(&source.name)
                );
                links
            });
            let _write_result = write!(
                output,
                "<article><span class=\"number\">{:02}</span><div><p class=\"kicker\">{}</p><h2>{}</h2><p>{}</p><p class=\"sources\">{links}</p></div></article>",
                index + 1,
                encode_text(&category_name(&story.primary_category)),
                encode_text(&story.headline),
                encode_text(&story.summary)
            );
            output
        },
    );
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"robots\" content=\"noindex,nofollow\"><title>Berlin Times · {}</title><style>{}</style></head><body><main><header><p>{} edition</p><h1>Berlin Times</h1><p>{}</p></header><section class=\"meta\"><span>Generated {}</span><span>Next edition {}</span><span>Schema v1 · {}</span></section><section>{stories}</section></main></body></html>",
        encode_text(&edition.display_date),
        STATUS_CSS,
        edition.edition_name.as_str(),
        encode_text(&edition.display_date),
        edition.generated_at.to_rfc3339(),
        edition.next_scheduled_at.to_rfc3339(),
        encode_text(&edition.edition_id)
    )
}

fn category_name(category: &crate::model::Category) -> String {
    serde_json::to_value(category)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "news".into())
        .replace('_', " ")
}

fn write(path: impl AsRef<Path>, bytes: &[u8]) -> Result<()> {
    let path = path.as_ref();
    fs::write(path, bytes).map_err(io_error(path))
}

fn replace_directory(output: &Path, staging: &Path, temporary: &Path) -> Result<()> {
    if output.exists() && !output.is_dir() {
        return Err(GeneratorError::Config(format!(
            "output path is not a directory: {}",
            output.display()
        )));
    }
    let backup = temporary.join("previous");
    if output.exists() {
        fs::rename(output, &backup).map_err(io_error(output))?;
    }
    if let Err(source) = fs::rename(staging, output) {
        if backup.exists() {
            let _restore_result = fs::rename(&backup, output);
        }
        return Err(GeneratorError::Io {
            path: output.into(),
            source,
        });
    }
    Ok(())
}

const STATUS_CSS: &str = r"
@font-face{font-family:Fraktur;src:url('assets/fonts/UnifrakturCook-Bold.ttf')}@font-face{font-family:Source;src:url('assets/fonts/SourceSerif4-Variable.ttf');font-weight:200 900}*{box-sizing:border-box}body{margin:0;background:#eeeae0;color:#171717;font-family:Source,Georgia,serif}main{max-width:900px;margin:0 auto;padding:48px 28px 80px;background:#fff;min-height:100vh}header{display:grid;grid-template-columns:1fr auto 1fr;align-items:end;border-top:5px solid;border-bottom:2px solid;padding:12px 0}header p{margin:0;text-transform:uppercase;letter-spacing:.12em;font:600 12px sans-serif}header p:last-child{text-align:right}h1{margin:0 28px;font:56px Fraktur,serif}.meta{display:flex;justify-content:space-between;gap:16px;padding:10px 0 16px;border-bottom:1px solid;font:11px sans-serif;text-transform:uppercase;letter-spacing:.08em}article{display:grid;grid-template-columns:48px 1fr;gap:18px;padding:26px 0;border-bottom:1px solid #444}.number{font:32px Source,serif;color:#777}.kicker,.sources{font:600 11px sans-serif;text-transform:uppercase;letter-spacing:.1em}h2{font-size:29px;line-height:1.05;margin:5px 0 10px}article p{font-size:18px;line-height:1.45;margin:0}.sources{margin-top:12px}.sources a{color:inherit}@media(max-width:650px){header{grid-template-columns:1fr;text-align:center;gap:6px}header p:last-child{text-align:center}h1{font-size:48px;order:-1}.meta{display:grid}article{grid-template-columns:34px 1fr}}
";

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::edition_id;
    use crate::model::ResearchEdition;

    #[test]
    fn edition_ids_are_deterministic() {
        let research = ResearchEdition {
            stories: Vec::new(),
            photo_candidates: Vec::new(),
        };
        let now = Utc.with_ymd_and_hms(2026, 8, 5, 4, 15, 0).single();
        let first = now.and_then(|value| edition_id(&research, value).ok());
        let second = now.and_then(|value| edition_id(&research, value).ok());
        assert_eq!(first, second);
        assert!(first.is_some_and(|value| value.starts_with("20260805T041500Z-")));
    }
}
