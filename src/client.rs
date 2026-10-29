//! Optional synchronous HTTP discovery of the two ClickOnce manifests.

use std::time::Duration;

use clickonce::{Manifest, ParseOptions};
use ureq::ResponseExt;

use crate::{Application, Deployment, Error, LIVE_DEPLOYMENT_URL, Url, location};

/// A synchronous HTTP client that downloads manifests to discover resource URLs.
///
/// Payloads are never fetched. The default client uses a 30-second timeout for
/// each manifest request and the ClickOnce parser's default input limits.
#[derive(Debug, Clone)]
pub struct Client {
    /// Reusable connections and HTTP request configuration.
    agent: ureq::Agent,
    /// Bounds applied while reading and parsing each manifest.
    options: ParseOptions,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Create an HTTP client with bounded requests and default parsing limits.
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .new_agent();
        Self::with_agent(agent)
    }

    /// Use a caller-configured HTTP agent, for example with a proxy or timeout.
    ///
    /// Unsuccessful HTTP statuses are rejected even if the supplied agent does
    /// not treat them as errors.
    pub fn with_agent(agent: ureq::Agent) -> Self {
        Self {
            agent,
            options: ParseOptions::default(),
        }
    }

    /// Customize the input byte and XML node limits for both manifests.
    pub fn with_parse_options(mut self, options: ParseOptions) -> Self {
        self.options = options;
        self
    }

    /// Discover the current live MTGO deployment and its resource URLs.
    ///
    /// # Errors
    /// Returns network, HTTP status, parsing, or resource-discovery errors.
    pub fn live(&self) -> Result<Application, Error> {
        self.discover(LIVE_DEPLOYMENT_URL)
    }

    /// Fetch a deployment and its referenced application manifest.
    ///
    /// Relative references use final response URLs after redirects. The
    /// deployment provider is retained as metadata rather than followed.
    ///
    /// # Errors
    /// Returns invalid URLs, network failures, unsuccessful status codes,
    /// exceeded parsing limits, or invalid or inconsistent manifest metadata.
    pub fn discover(&self, url: &str) -> Result<Application, Error> {
        let (url, manifest) = self.fetch_manifest(&Url::parse(url)?)?;
        let deployment = Deployment::from_manifest(url, manifest)?;
        let (url, manifest) = self.fetch_manifest(deployment.application_manifest_url())?;
        deployment.with_application(url, manifest)
    }

    /// Read a bounded manifest response and retain its final source URL.
    fn fetch_manifest(&self, url: &Url) -> Result<(Url, Manifest), Error> {
        location::check_url(url)?;
        let mut response = self.agent.get(url.as_str()).call()?;
        if !response.status().is_success() {
            return Err(ureq::Error::StatusCode(response.status().as_u16()).into());
        }
        let url = Url::parse(&response.get_uri().to_string())?;
        location::check_url(&url)?;
        let manifest = self.options.from_reader(response.body_mut().as_reader())?;
        Ok((url, manifest))
    }
}

/// HTTP transport types for constructing a custom [`Client`] agent.
pub use ureq as transport;
