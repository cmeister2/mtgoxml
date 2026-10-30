//! Optional synchronous HTTP discovery of the two ClickOnce manifests.

use std::collections::BTreeSet;
use std::time::Duration;

use clickonce::{Manifest, ParseOptions};
use ureq::ResponseExt;

use crate::cardstyle::CardStyles;
use crate::{
    Application, ArtworkResolver, Deployment, Error, LIVE_DEPLOYMENT_URL, Resource, ResourceKind,
    Url, carddata, location,
};
use carddata::{CardDataFile, CardDatabase, FieldValue};

/// A synchronous HTTP client that downloads manifests to discover resource URLs.
///
/// Discovery fetches only manifests. Card database loading is a separate,
/// explicit operation. Each request has a 30-second timeout by default.
#[derive(Debug, Clone)]
pub struct Client {
    /// Reusable connections and HTTP request configuration.
    agent: ureq::Agent,
    /// Bounds applied while reading and parsing each manifest.
    options: ParseOptions,
    /// Bounds applied while reading and parsing each CardDataSource component.
    card_data_options: carddata::ParseOptions,
    /// Bounds for auxiliary XML documents such as `CardStyle.xml`.
    client_data_options: crate::cardstyle::ParseOptions,
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
            card_data_options: carddata::ParseOptions::default(),
            client_data_options: crate::cardstyle::ParseOptions::default(),
        }
    }

    /// Customize the input byte and XML node limits for both manifests.
    pub fn with_parse_options(mut self, options: ParseOptions) -> Self {
        self.options = options;
        self
    }

    /// Customize byte and XML node limits for individual CardDataSource files.
    pub fn with_card_data_options(mut self, options: carddata::ParseOptions) -> Self {
        self.card_data_options = options;
        self
    }

    /// Customize encoded-byte, decoded-byte, and node limits for auxiliary XML.
    pub fn with_client_data_options(mut self, options: crate::cardstyle::ParseOptions) -> Self {
        self.client_data_options = options;
        self
    }

    /// Download the captured application's `CardStyle.xml` declarations.
    ///
    /// Style inheritance is resolved explicitly through `CardStyles::resolve`;
    /// missing bases in the source do not prevent parsing other styles. Asset
    /// names remain opaque references. Resource hashes are retained by the
    /// application catalog but are not verified by this convenience loader.
    ///
    /// # Errors
    /// Returns an error for a missing resource, invalid URL, failed request or
    /// HTTP status, parsing limits, unsupported encoding, or invalid XML data.
    pub fn load_card_styles(&self, application: &Application) -> Result<CardStyles, Error> {
        let bytes = self.fetch_client_data(application, "CardStyle.xml")?;
        CardStyles::from_bytes_with_options(&bytes, self.client_data_options).map_err(Error::from)
    }

    /// Fetch one bounded auxiliary document from the supplied application catalog.
    fn fetch_client_data(&self, application: &Application, path: &str) -> Result<Vec<u8>, Error> {
        let resource = application
            .resource(path)
            .filter(|resource| resource.kind == ResourceKind::File)
            .ok_or_else(|| Error::MissingResource(path.into()))?;
        location::check_url(&resource.url)?;
        let mut response = self.agent.get(resource.url.as_str()).call()?;
        if !response.status().is_success() {
            return Err(ureq::Error::StatusCode(response.status().as_u16()).into());
        }
        location::check_url(&Url::parse(&response.get_uri().to_string())?)?;
        self.client_data_options
            .read(response.body_mut().as_reader())
            .map_err(Error::from)
    }

    /// Load the application's `MTGO.exe.config` artwork download configuration.
    ///
    /// Reads at most 16 MiB and resolves relative artwork roots against the
    /// configuration response's final URL. This downloads configuration only;
    /// artwork URLs are constructed without downloading or checking images.
    /// Declared resource hashes are retained by the catalog but not verified by
    /// this client, consistently with card-data loading.
    ///
    /// # Errors
    /// Returns an error for a missing configuration resource, invalid network
    /// location, request or status failure, exceeded response limit, or invalid
    /// artwork configuration.
    pub fn load_artwork(&self, application: &Application) -> Result<ArtworkResolver, Error> {
        let resource = application
            .resource("MTGO.exe.config")
            .filter(|resource| resource.kind == ResourceKind::File)
            .ok_or_else(|| Error::MissingResource("MTGO.exe.config".into()))?;
        location::check_url(&resource.url)?;
        let mut response = self.agent.get(resource.url.as_str()).call()?;
        if !response.status().is_success() {
            return Err(ureq::Error::StatusCode(response.status().as_u16()).into());
        }
        let url = Url::parse(&response.get_uri().to_string())?;
        location::check_url(&url)?;
        let bytes = response
            .body_mut()
            .with_config()
            .limit(crate::artwork::MAX_CONFIG_BYTES as u64)
            .read_to_vec()?;
        ArtworkResolver::from_config(url, &bytes).map_err(Error::from)
    }

    /// Load and validate every CardDataSource component in this application.
    ///
    /// This explicitly downloads the full dataset, which can exceed 100 MiB.
    /// The master XML and DTD are skipped: individual components are already
    /// listed in the manifest, so external entity expansion is unnecessary.
    ///
    /// # Errors
    /// Returns network, status, parsing, duplicate, missing-reference, clone
    /// cycle, or invalid-property errors. The database is returned only after
    /// all objects resolve successfully.
    pub fn load_card_database(&self, application: &Application) -> Result<CardDatabase, Error> {
        let mut database = application.card_database();
        for resource in application
            .card_data_resources()
            .filter(|resource| is_component(resource))
        {
            let file = self.fetch_card_data(resource)?;
            database.insert(application, file)?;
        }
        database.validate()?;
        Ok(database)
    }

    /// Load selected sets, their object fragments, and referenced lookup tables.
    ///
    /// Set codes are case insensitive. Both `client_CODE.xml` and
    /// `client_CODE_DO.xml` are included when present. No payloads are fetched
    /// until all requested codes have been found in the supplied manifest.
    /// If a clone's base is in another set, include that set as well; missing
    /// bases produce an error identifying the required object ID.
    ///
    /// # Errors
    /// Returns an error for an empty selection, absent sets or tables, failed
    /// requests, invalid XML, or objects that cannot be fully resolved.
    pub fn load_card_sets(
        &self,
        application: &Application,
        sets: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<CardDatabase, Error> {
        let codes: BTreeSet<_> = sets
            .into_iter()
            .map(|code| code.as_ref().to_ascii_uppercase())
            .collect();
        if codes.is_empty() {
            return Err(carddata::Error::InvalidData("select at least one set".into()).into());
        }
        let mut selected = BTreeSet::new();
        for code in codes {
            // Collect every matching fragment before issuing any HTTP requests.
            let mut found = false;
            for name in [
                format!("CardDataSource/client_{code}.xml"),
                format!("CardDataSource/client_{code}_DO.xml"),
            ] {
                if let Some(resource) = application
                    .resource(&name)
                    .filter(|resource| is_component(resource))
                {
                    found = true;
                    selected.insert(resource.path.clone());
                }
            }
            if !found {
                return Err(Error::MissingCardDataResource(code));
            }
        }
        let mut database = application.card_database();
        let mut tables = BTreeSet::new();
        for path in selected {
            // Inspect the set's reference fields to request only its lookup tables.
            let resource = application
                .resource(&path)
                .ok_or_else(|| Error::MissingCardDataResource(path.clone()))?;
            let file = self.fetch_card_data(resource)?;
            if let CardDataFile::Set(set) = &file {
                for object in &set.objects {
                    collect_tables(&object.fields, &mut tables);
                }
            }
            database.insert(application, file)?;
        }
        for table in tables {
            // Lookup IDs are opaque; each property names the table to load.
            let path = format!("CardDataSource/{table}.xml");
            let resource = application
                .resource(&path)
                .filter(|resource| is_component(resource))
                .ok_or_else(|| Error::MissingCardDataResource(path.clone()))?;
            let file = self.fetch_card_data(resource)?;
            database.insert(application, file)?;
        }
        database.validate()?;
        Ok(database)
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

    /// Fetch a bounded component and check its declared table or set identity.
    fn fetch_card_data(&self, resource: &Resource) -> Result<CardDataFile, Error> {
        location::check_url(&resource.url)?;
        let mut response = self.agent.get(resource.url.as_str()).call()?;
        if !response.status().is_success() {
            return Err(ureq::Error::StatusCode(response.status().as_u16()).into());
        }
        let file = self
            .card_data_options
            .from_reader(response.body_mut().as_reader())?;
        let filename = resource.path.rsplit('/').next().unwrap_or_default();
        let matches = match &file {
            CardDataFile::Set(set) => {
                filename.eq_ignore_ascii_case(&format!("client_{}.xml", set.id))
                    || filename.eq_ignore_ascii_case(&format!("client_{}_DO.xml", set.id))
            }
            CardDataFile::Lookup(table) => {
                filename.eq_ignore_ascii_case(&format!("{}.xml", table.name))
            }
        };
        if !matches {
            return Err(carddata::Error::InvalidData(format!(
                "component identity does not match {}",
                resource.path
            ))
            .into());
        }
        Ok(file)
    }
}

/// Select component XML files while excluding the external-entity master index.
fn is_component(resource: &Resource) -> bool {
    resource.kind == ResourceKind::File
        && resource.is_xml()
        && !resource
            .path
            .rsplit('/')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("MTGOV3CardData.xml"))
}

/// Collect table names from both ordinary fields and nested attribute groups.
fn collect_tables(
    fields: &std::collections::BTreeMap<String, FieldValue>,
    tables: &mut BTreeSet<String>,
) {
    for (name, value) in fields {
        match value {
            FieldValue::Reference(_) => {
                tables.insert(name.clone());
            }
            FieldValue::Fields(fields) => collect_tables(fields, tables),
            FieldValue::Literal(_) | FieldValue::Flag => {}
        }
    }
}

/// HTTP transport types for constructing a custom [`Client`] agent.
pub use ureq as transport;
