//! Available product artwork from the client's configured Store2 directory.

use std::collections::BTreeMap;

use mtgoxml::{ArtworkResolver, Url};
use ureq::ResponseExt;

use super::Result;

/// Resolve each generated filename once without downloading image bodies.
pub(super) struct StoreImages {
    /// Optional Store2 location selected by the client configuration.
    base_url: Option<Url>,
    /// Bounded HTTP client shared with the surrounding export.
    agent: ureq::Agent,
    /// Successful and unavailable outcomes for filenames already requested.
    cache: BTreeMap<String, Option<String>>,
}

impl StoreImages {
    /// Retain this snapshot's configured store directory and HTTP client.
    pub(super) fn new(artwork: &ArtworkResolver, agent: &ureq::Agent) -> Self {
        Self {
            base_url: artwork.store2_base_url().cloned(),
            agent: agent.clone(),
            cache: BTreeMap::new(),
        }
    }

    /// Return an available image URL, or null for empty names and missing images.
    ///
    /// # Errors
    ///
    /// Errors stop the export when the request fails or returns an unexpected
    /// status or content type. Only HTTP 404 and 410 mean an unavailable image.
    pub(super) fn resolve(&mut self, image_name: Option<&str>) -> Result<Option<String>> {
        let Some(name) = image_name.filter(|name| !name.is_empty()) else {
            return Ok(None);
        };
        if let Some(cached) = self.cache.get(name) {
            return Ok(cached.clone());
        }
        let result = self.request(name)?;
        self.cache.insert(name.to_owned(), result.clone());
        Ok(result)
    }

    /// Probe the configured Store2 location while preserving the exact filename.
    fn request(&self, name: &str) -> Result<Option<String>> {
        let Some(mut url) = self.base_url.clone() else {
            return Ok(None);
        };
        url.path_segments_mut()
            .map_err(|()| "store image URL does not support path segments")?
            .pop_if_empty()
            .push(name);
        let url = transport_url(&url)?;
        let response = self
            .agent
            .head(url.as_str())
            .config()
            .http_status_as_error(false)
            .build()
            .call()?;
        if matches!(response.status().as_u16(), 404 | 410) {
            return Ok(None);
        }
        if !response.status().is_success() || matches!(response.status().as_u16(), 204 | 205) {
            return Err(format!("HTTP {} for store image {url}", response.status()).into());
        }
        let final_url = Url::parse(&response.get_uri().to_string())?;
        transport_url(&final_url)?;
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let media_type = content_type.split(';').next().unwrap_or_default().trim();
        if !media_type.to_ascii_lowercase().starts_with("image/") || media_type.len() <= 6 {
            return Err(format!("expected an image content type for {final_url}").into());
        }
        Ok(Some(final_url.to_string()))
    }
}

/// Restrict artwork locations and use HTTPS for the publisher's graphics endpoint.
fn transport_url(url: &Url) -> Result<Url> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("invalid store image URL".into());
    }
    let mut url = url.clone();
    if url.scheme() == "http" && url.host_str() == Some("mtgo.patch.daybreakgames.com") {
        url.set_scheme("https")
            .map_err(|()| "could not upgrade publisher URL")?;
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};
    use std::net::TcpListener;
    use std::time::Duration;

    /// Configure both store paths to expose any accidental use of the older directory.
    fn resolver(root: &Url, newer: bool) -> Result<StoreImages> {
        let optional = if newer {
            r#"<store2ArtSubPath value="Store2"/>"#
        } else {
            ""
        };
        let artwork = ArtworkResolver::from_config(
            Url::parse("https://patch.example/MTGO.exe.config")?,
            format!(r#"<configuration><downloadSource><artRoot value="{root}"/><cardArtSubPath value="Cards/Pics"/><storeArtSubPath value="Store"/>{optional}</downloadSource></configuration>"#).as_bytes(),
        )?;
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(2)))
            .build()
            .new_agent();
        Ok(StoreImages::new(&artwork, &agent))
    }

    /// Exact casing, escaped filename segments, query strings, and cached hits survive HEAD.
    #[test]
    fn preserves_names_queries_and_caches_available_urls() -> Result<()> {
        let mut server = Server::new();
        let image = server
            .mock(
                "HEAD",
                "/Graphics/Store2/MTGOTHER%20folder%2Fa%3F%23.png?token=test",
            )
            .with_status(200)
            .with_header("content-type", "image/png")
            .with_header("content-length", "234352")
            .expect(1)
            .create();
        let root = Url::parse(&format!("{}/Graphics?token=test", server.url()))?;
        let mut images = resolver(&root, true)?;
        let result = images.resolve(Some("MTGOTHER folder/a?#.png"))?;
        assert_eq!(images.resolve(Some("MTGOTHER folder/a?#.png"))?, result);
        assert_eq!(
            result,
            Some(format!(
                "{}/Graphics/Store2/MTGOTHER%20folder%2Fa%3F%23.png?token=test",
                server.url()
            ))
        );
        image.assert();
        Ok(())
    }

    /// Missing images return null, reuse cached misses, and never probe the old Store path.
    #[test]
    fn caches_not_found_and_gone_images_without_store_fallback() -> Result<()> {
        for status in [404, 410] {
            let mut server = Server::new();
            let image = server
                .mock("HEAD", "/Graphics/Store2/missing.png")
                .with_status(status)
                .expect(1)
                .create();
            let old_store = server
                .mock("HEAD", "/Graphics/Store/missing.png")
                .with_status(200)
                .with_header("content-type", "image/png")
                .expect(0)
                .create();
            let root = Url::parse(&format!("{}/Graphics", server.url()))?;
            let mut images = resolver(&root, true)?;
            assert_eq!(images.resolve(Some("missing.png"))?, None);
            assert_eq!(images.resolve(Some("missing.png"))?, None);
            image.assert();
            old_store.assert();
        }
        Ok(())
    }

    /// The generated URL identifies the image response after HTTP redirects.
    #[test]
    fn returns_final_image_url_after_redirects() -> Result<()> {
        let mut server = Server::new();
        let redirect = server
            .mock("HEAD", "/Graphics/Store2/redirect.png")
            .with_status(302)
            .with_header("location", "/final/actual.PNG")
            .expect(1)
            .create();
        let image = server
            .mock("HEAD", "/final/actual.PNG")
            .with_status(200)
            .with_header("content-type", "Image/PNG; charset=binary")
            .expect(1)
            .create();
        let root = Url::parse(&format!("{}/Graphics", server.url()))?;
        let mut images = resolver(&root, true)?;
        assert_eq!(
            images.resolve(Some("redirect.png"))?,
            Some(format!("{}/final/actual.PNG", server.url()))
        );
        redirect.assert();
        image.assert();
        Ok(())
    }

    /// Unexpected statuses and invalid content types fail without caching or fallback.
    #[test]
    fn errors_remain_errors_and_can_be_retried() -> Result<()> {
        for (status, content_type) in [
            (503, None),
            (403, None),
            (429, None),
            (204, Some("image/png")),
            (205, Some("image/png")),
            (200, Some("text/html")),
            (200, Some("image/")),
            (200, None),
        ] {
            let mut server = Server::new();
            let failure = server
                .mock("HEAD", "/Graphics/Store2/retry.png")
                .with_status(status)
                .expect(1);
            let failure = if let Some(value) = content_type {
                failure.with_header("content-type", value)
            } else {
                failure
            }
            .create();
            let root = Url::parse(&format!("{}/Graphics", server.url()))?;
            let mut images = resolver(&root, true)?;
            assert!(images.resolve(Some("retry.png")).is_err());
            assert!(!images.cache.contains_key("retry.png"));
            failure.assert();
            failure.remove();
            let image = server
                .mock("HEAD", "/Graphics/Store2/retry.png")
                .with_status(200)
                .with_header("content-type", "image/png")
                .expect(1)
                .create();
            assert!(images.resolve(Some("retry.png"))?.is_some());
            image.assert();
        }
        Ok(())
    }

    /// A refused connection stops resolution and leaves the filename absent from the cache.
    #[test]
    fn network_failure_does_not_become_a_missing_image() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        drop(listener);
        let root = Url::parse(&format!("http://{address}/Graphics"))?;
        let mut images = resolver(&root, true)?;
        let error = images.resolve(Some("disconnect.png")).unwrap_err();
        assert!(error.downcast_ref::<ureq::Error>().is_some(), "{error}");
        assert!(!images.cache.contains_key("disconnect.png"));
        Ok(())
    }

    /// Empty names and absent Store2 configuration never contact the available Store path.
    #[test]
    fn skips_empty_names_and_missing_directories() -> Result<()> {
        let mut server = Server::new();
        let unused = server.mock("HEAD", Matcher::Any).expect(0).create();
        let root = Url::parse(&format!("{}/Graphics", server.url()))?;
        let mut images = resolver(&root, true)?;
        assert_eq!(images.resolve(None)?, None);
        assert_eq!(images.resolve(Some(""))?, None);
        let mut images = resolver(&root, false)?;
        assert_eq!(images.resolve(Some("unused.png"))?, None);
        unused.assert();
        Ok(())
    }

    /// Transport rules upgrade the official endpoint and reject unsupported locations.
    #[test]
    fn upgrades_official_http_urls_and_rejects_invalid_locations() -> Result<()> {
        let official =
            "http://mtgo.patch.daybreakgames.com/patch/mtg/live/Graphics/Store2/MTGOTHER.png";
        assert_eq!(
            transport_url(&Url::parse(official)?)?.as_str(),
            official.replacen("http:", "https:", 1)
        );
        for input in [
            "file:///tmp/image.png",
            "https://user:password@art.example/image.png",
            "https://art.example/image.png#fragment",
        ] {
            assert!(transport_url(&Url::parse(input)?).is_err());
        }
        Ok(())
    }
}
