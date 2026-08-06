use std::{
    io::Cursor,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use futures_util::StreamExt;
use image::{DynamicImage, ImageReader, codecs::jpeg::JpegEncoder, imageops::FilterType};
use reqwest::header;
use scraper::{Html, Selector};
use tokio::net::lookup_host;
use tracing::warn;
use url::Url;

use crate::{
    error::{GeneratorError, Result},
    model::{ResearchEdition, ResearchStory},
};

const MAX_REDIRECTS: usize = 5;
const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 12 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 32_000_000;
const OUTPUT_WIDTH: u32 = 1_120;
const OUTPUT_HEIGHT: u32 = 734;

#[derive(Debug)]
pub struct PhotoAsset {
    pub bytes: Vec<u8>,
    pub story_id: String,
    pub alt: String,
    pub credit: String,
    pub source_page_url: String,
}

#[derive(Clone, Default)]
pub struct SafeHttp {
    #[cfg(test)]
    test_client: Option<reqwest::Client>,
}

struct Fetched {
    final_url: Url,
    content_type: Option<String>,
    bytes: Vec<u8>,
}

impl SafeHttp {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            #[cfg(test)]
            test_client: None,
        }
    }

    pub async fn build_lead_photo(&self, edition: &ResearchEdition) -> Result<PhotoAsset> {
        let mut failures = Vec::new();
        let candidates = edition
            .photo_candidates
            .iter()
            .filter_map(|candidate| {
                edition
                    .stories
                    .iter()
                    .find(|story| story.id == *candidate)
                    .or_else(|| {
                        failures.push(format!("candidate {candidate} was not found"));
                        None
                    })
            })
            .flat_map(|story| story.sources.iter().map(move |source| (story, source)))
            .collect::<Vec<_>>();

        for (story, source) in candidates {
            if let Some(image_url) = story.image_url.as_deref() {
                match self
                    .photo_from_image(story, &source.name, &source.url, image_url)
                    .await
                {
                    Ok(photo) => return Ok(photo),
                    Err(error) => {
                        warn!(story_id = %story.id, image_url, %error, "Exa image candidate rejected");
                        failures.push(format!("{} direct image: {error}", story.id));
                    }
                }
            }
            match self
                .photo_from_source(story, &source.name, &source.url)
                .await
            {
                Ok(photo) => return Ok(photo),
                Err(error) => {
                    warn!(story_id = %story.id, source_url = %source.url, %error, "photo candidate rejected");
                    failures.push(format!("{} via {}: {error}", story.id, source.name));
                }
            }
        }
        Err(GeneratorError::NoPhoto(failures.join("; ")))
    }

    pub async fn fetch_published_photo(&self, image_url: &str) -> Result<Vec<u8>> {
        let image_url = Url::parse(image_url)
            .map_err(|error| GeneratorError::UnsafeUrl(format!("invalid image url: {error}")))?;
        let image = self.fetch(image_url, MAX_IMAGE_BYTES, "image/jpeg").await?;
        if !image
            .content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("image/jpeg"))
        {
            return Err(GeneratorError::NoPhoto(
                "published photo is not a jpeg".into(),
            ));
        }
        let reader = ImageReader::new(Cursor::new(&image.bytes))
            .with_guessed_format()
            .map_err(|error| GeneratorError::Image(error.to_string()))?;
        let dimensions = reader
            .into_dimensions()
            .map_err(|error| GeneratorError::Image(error.to_string()))?;
        if dimensions != (OUTPUT_WIDTH, OUTPUT_HEIGHT) {
            return Err(GeneratorError::Image(
                "published photo has unexpected dimensions".into(),
            ));
        }
        Ok(image.bytes)
    }

    async fn photo_from_image(
        &self,
        story: &ResearchStory,
        source_name: &str,
        source_url: &str,
        image_url: &str,
    ) -> Result<PhotoAsset> {
        let image_url = Url::parse(image_url)
            .map_err(|error| GeneratorError::UnsafeUrl(format!("invalid image url: {error}")))?;
        let image = self
            .fetch(
                image_url,
                MAX_IMAGE_BYTES,
                "image/jpeg,image/png,image/webp",
            )
            .await?;
        if !image
            .content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("image/"))
        {
            return Err(GeneratorError::NoPhoto(
                "direct resource is not image".into(),
            ));
        }
        Ok(PhotoAsset {
            bytes: process_image(&image.bytes, true)?,
            story_id: story.id.clone(),
            alt: format!("News photograph for {}", story.headline),
            credit: source_name.into(),
            source_page_url: source_url.into(),
        })
    }

    async fn photo_from_source(
        &self,
        story: &ResearchStory,
        source_name: &str,
        source_url: &str,
    ) -> Result<PhotoAsset> {
        let page_url = Url::parse(source_url)
            .map_err(|error| GeneratorError::UnsafeUrl(format!("invalid source url: {error}")))?;
        let page = self
            .fetch(page_url, MAX_PAGE_BYTES, "text/html,application/xhtml+xml")
            .await?;
        if !page
            .content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("text/html"))
        {
            return Err(GeneratorError::NoPhoto(
                "source page did not return html".into(),
            ));
        }

        let html = String::from_utf8_lossy(&page.bytes);
        let document = Html::parse_document(&html);
        let image_value = meta_content(&document, "property", "og:image:secure_url")
            .or_else(|| meta_content(&document, "property", "og:image"))
            .ok_or_else(|| GeneratorError::NoPhoto("source page has no open graph image".into()))?;
        let image_url = page.final_url.join(&image_value).map_err(|error| {
            GeneratorError::UnsafeUrl(format!("invalid open graph image url: {error}"))
        })?;
        let image = self
            .fetch(
                image_url,
                MAX_IMAGE_BYTES,
                "image/jpeg,image/png,image/webp",
            )
            .await?;
        if !image
            .content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("image/"))
        {
            return Err(GeneratorError::NoPhoto(
                "open graph resource is not an image".into(),
            ));
        }
        let bytes = process_image(&image.bytes, true)?;
        Ok(PhotoAsset {
            bytes,
            story_id: story.id.clone(),
            alt: format!("News photograph for {}", story.headline),
            credit: source_name.into(),
            source_page_url: source_url.into(),
        })
    }

    async fn fetch(&self, mut url: Url, limit: usize, accept: &str) -> Result<Fetched> {
        #[cfg(test)]
        if let Some(client) = &self.test_client {
            return fetch_for_test(client, url, limit, accept).await;
        }
        for redirects in 0..=MAX_REDIRECTS {
            let client = client_for_public_url(&url).await?;
            let response = client
                .get(url.clone())
                .header(header::ACCEPT, accept)
                .send()
                .await?;
            if response.status().is_redirection() {
                if redirects == MAX_REDIRECTS {
                    return Err(GeneratorError::UnsafeUrl("redirect limit exceeded".into()));
                }
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        GeneratorError::UnsafeUrl("redirect is missing location".into())
                    })?;
                url = url.join(location).map_err(|error| {
                    GeneratorError::UnsafeUrl(format!("invalid redirect url: {error}"))
                })?;
                continue;
            }
            if !response.status().is_success() {
                return Err(GeneratorError::ApiStatus {
                    status: response.status().as_u16(),
                    message: "photo source request failed".into(),
                });
            }
            if response
                .content_length()
                .is_some_and(|length| usize::try_from(length).map_or(true, |length| length > limit))
            {
                return Err(GeneratorError::NoPhoto(
                    "remote response exceeds size limit".into(),
                ));
            }
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_ascii_lowercase);
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                if bytes.len().saturating_add(chunk.len()) > limit {
                    return Err(GeneratorError::NoPhoto(
                        "remote response exceeds size limit".into(),
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok(Fetched {
                final_url: url,
                content_type,
                bytes,
            });
        }
        Err(GeneratorError::UnsafeUrl("redirect handling failed".into()))
    }
}

#[cfg(test)]
async fn fetch_for_test(
    client: &reqwest::Client,
    url: Url,
    limit: usize,
    accept: &str,
) -> Result<Fetched> {
    let response = client
        .get(url.clone())
        .header(header::ACCEPT, accept)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(GeneratorError::ApiStatus {
            status: response.status().as_u16(),
            message: "photo source request failed".into(),
        });
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_ascii_lowercase);
    let bytes = response.bytes().await?;
    if bytes.len() > limit {
        return Err(GeneratorError::NoPhoto(
            "remote response exceeds size limit".into(),
        ));
    }
    Ok(Fetched {
        final_url: url,
        content_type,
        bytes: bytes.into(),
    })
}

pub fn fixture_photo(
    bytes: &[u8],
    story: &ResearchStory,
    source_name: &str,
    source_page_url: &str,
) -> Result<PhotoAsset> {
    Ok(PhotoAsset {
        bytes: process_image(bytes, false)?,
        story_id: story.id.clone(),
        alt: format!("News photograph for {}", story.headline),
        credit: source_name.into(),
        source_page_url: source_page_url.into(),
    })
}

fn process_image(bytes: &[u8], enforce_dimensions: bool) -> Result<Vec<u8>> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| GeneratorError::Image(error.to_string()))?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| GeneratorError::Image(error.to_string()))?;
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if pixels > MAX_IMAGE_PIXELS {
        return Err(GeneratorError::Image(
            "image dimensions exceed pixel limit".into(),
        ));
    }
    if enforce_dimensions && (width < 640 || height < 360) {
        return Err(GeneratorError::Image(
            "image dimensions are too small".into(),
        ));
    }

    let decoded = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| GeneratorError::Image(error.to_string()))?
        .decode()
        .map_err(|error| GeneratorError::Image(error.to_string()))?;
    let grayscale = decoded.grayscale();
    let processed = if enforce_dimensions {
        grayscale.adjust_contrast(8.0).resize_to_fill(
            OUTPUT_WIDTH,
            OUTPUT_HEIGHT,
            FilterType::Lanczos3,
        )
    } else {
        grayscale.resize_to_fill(OUTPUT_WIDTH, OUTPUT_HEIGHT, FilterType::Nearest)
    };
    encode_jpeg(&processed)
}

fn encode_jpeg(image: &DynamicImage) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    JpegEncoder::new_with_quality(&mut output, 84)
        .encode_image(image)
        .map_err(|error| GeneratorError::Image(error.to_string()))?;
    Ok(output)
}

fn meta_content(document: &Html, attribute: &str, value: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[{attribute}=\"{value}\"]")).ok()?;
    document
        .select(&selector)
        .find_map(|element| element.value().attr("content"))
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .map(Into::into)
}

async fn client_for_public_url(url: &Url) -> Result<reqwest::Client> {
    if url.scheme() != "https" {
        return Err(GeneratorError::UnsafeUrl(
            "only https urls are permitted".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(GeneratorError::UnsafeUrl(
            "url credentials are not permitted".into(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| GeneratorError::UnsafeUrl("url has no host".into()))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| GeneratorError::UnsafeUrl("url does not have a known https port".into()))?;
    if port != 443 {
        return Err(GeneratorError::UnsafeUrl(
            "non-standard https ports are not permitted".into(),
        ));
    }
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(GeneratorError::UnsafeUrl(
            "localhost is not permitted".into(),
        ));
    }

    if let Ok(address) = host.parse::<IpAddr>() {
        if is_public_ip(address) {
            return http_client_builder().build().map_err(GeneratorError::from);
        }
        return Err(GeneratorError::UnsafeUrl(
            "private or reserved address is not permitted".into(),
        ));
    }

    let addresses = lookup_host((host, port))
        .await
        .map_err(|error| GeneratorError::UnsafeUrl(format!("dns lookup failed: {error}")))?
        .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(GeneratorError::UnsafeUrl(
            "host resolves to a private or reserved address".into(),
        ));
    }
    let pinned = addresses
        .iter()
        .map(|address| SocketAddr::new(address.ip(), port))
        .collect::<Vec<_>>();
    http_client_builder()
        .resolve_to_addrs(host, &pinned)
        .build()
        .map_err(GeneratorError::from)
}

fn http_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(18))
        .user_agent("berlin-times/0.1 (+https://pr0me.github.io/trmnl-apps/)")
}

fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [first, second, third, _] = address.octets();
    !(first == 0
        || first == 10
        || first == 127
        || first >= 224
        || (first == 100 && (64..=127).contains(&second))
        || (first == 169 && second == 254)
        || (first == 172 && (16..=31).contains(&second))
        || (first == 192 && second == 0 && third == 0)
        || (first == 192 && second == 0 && third == 2)
        || (first == 192 && second == 168)
        || (first == 198 && (second == 18 || second == 19))
        || (first == 198 && second == 51 && third == 100)
        || (first == 203 && second == 0 && third == 113))
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }
    let segments = address.segments();
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use chrono::{DateTime, Utc};
    use scraper::Html;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::{SafeHttp, is_public_ip, meta_content, process_image};
    use crate::model::{Category, ResearchEdition, ResearchSource, ResearchStory};

    fn image_bytes() -> Vec<u8> {
        let mut bytes = b"P6\n640 360\n255\n".to_vec();
        bytes.resize(bytes.len() + (640 * 360 * 3), 127);
        bytes
    }

    fn edition(server: &MockServer, image_path: Option<&str>) -> ResearchEdition {
        let source_url = format!("{}/article", server.uri());
        let image_url = image_path.map(|path| format!("{}{path}", server.uri()));
        ResearchEdition {
            stories: vec![ResearchStory {
                id: "test-story".into(),
                primary_category: Category::World,
                is_developing: false,
                headline: "Test story headline".into(),
                summary: "Test story summary.".into(),
                published_at: DateTime::<Utc>::UNIX_EPOCH,
                sources: vec![ResearchSource {
                    name: "Reuters".into(),
                    url: source_url,
                }],
                image_url,
            }],
            photo_candidates: vec!["test-story".into()],
        }
    }

    fn test_http() -> SafeHttp {
        SafeHttp {
            test_client: Some(reqwest::Client::new()),
        }
    }

    #[test]
    fn rejects_private_and_documentation_addresses() {
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))));
        assert!(!is_public_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_public_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
    }

    #[test]
    fn rejects_images_exceeding_pixel_limit() {
        let image = b"P6\n6000 6000\n255\n";
        let result = process_image(image, true);
        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("pixel limit"))
        );
    }

    #[test]
    fn reports_missing_open_graph_metadata() {
        let document = Html::parse_document("<html><head></head><body></body></html>");
        assert!(meta_content(&document, "property", "og:image").is_none());
    }

    #[tokio::test]
    async fn uses_direct_exa_image() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/direct"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/x-portable-pixmap")
                    .set_body_bytes(image_bytes()),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/article"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let photo = test_http()
            .build_lead_photo(&edition(&server, Some("/direct")))
            .await?;
        assert_eq!(photo.story_id, "test-story");
        assert_eq!(photo.credit, "Reuters");
        assert!(photo.source_page_url.ends_with("/article"));
        Ok(())
    }

    #[tokio::test]
    async fn validates_published_photo() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let expected = process_image(&image_bytes(), true)?;
        Mock::given(method("GET"))
            .and(path("/published.jpg"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/jpeg")
                    .set_body_bytes(expected.clone()),
            )
            .mount(&server)
            .await;

        let actual = test_http()
            .fetch_published_photo(&format!("{}/published.jpg", server.uri()))
            .await?;
        assert_eq!(actual, expected);
        Ok(())
    }

    #[tokio::test]
    async fn falls_back_to_open_graph_after_invalid_exa_image()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/invalid"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "text/plain")
                    .set_body_string("not an image"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/article"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    "<meta property=\"og:image\" content=\"{}/fallback\">",
                    server.uri()
                ),
                "text/html",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/fallback"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/x-portable-pixmap")
                    .set_body_bytes(image_bytes()),
            )
            .mount(&server)
            .await;

        let photo = test_http()
            .build_lead_photo(&edition(&server, Some("/invalid")))
            .await?;
        assert_eq!(photo.story_id, "test-story");
        Ok(())
    }

    #[tokio::test]
    async fn selects_later_story_with_usable_image()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/article"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("<html></html>", "text/html"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/direct"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/x-portable-pixmap")
                    .set_body_bytes(image_bytes()),
            )
            .mount(&server)
            .await;
        let mut edition = edition(&server, None);
        let mut second = edition
            .stories
            .first()
            .cloned()
            .ok_or("test edition must contain story")?;
        second.id = "second-story".into();
        second.image_url = Some(format!("{}/direct", server.uri()));
        edition.stories.push(second);
        edition.photo_candidates.push("second-story".into());

        let photo = test_http().build_lead_photo(&edition).await?;

        assert_eq!(photo.story_id, "second-story");
        Ok(())
    }

    #[tokio::test]
    async fn fails_when_no_selected_story_has_usable_image()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/article"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("<html></html>", "text/html"))
            .mount(&server)
            .await;
        assert!(
            test_http()
                .build_lead_photo(&edition(&server, None))
                .await
                .is_err()
        );
        Ok(())
    }
}
