//! Local HTTP integration coverage for optional manifest discovery.

#![cfg(feature = "http")]

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use mtgoxml::{Application, Client, Deployment, Error, carddata, clickonce, transport};

/// A deployment whose application reference is deliberately relative.
const DEPLOYMENT: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity name="MTGO.application" version="1.2.3.4" />
  <deployment install="true"><deploymentProvider codebase="https://unused.example.test/MTGO.application" /></deployment>
  <dependency><dependentAssembly dependencyType="install" codebase="version\MTGO.exe.manifest">
    <assemblyIdentity name="MTGO.exe" version="1.2.3.4" />
  </dependentAssembly></dependency>
</assembly>"#;

/// An application exposing one XML resource, without requiring payload requests.
const APPLICATION: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity name="MTGO.exe" version="1.2.3.4" />
  <application /><file name="CardDataSource\Cards.xml" size="4" />
</assembly>"#;

/// A bounded local server that records every requested URI for later assertions.
struct Server {
    /// The local origin, including its dynamically assigned port.
    origin: String,
    /// Paths recorded before responses are written.
    requests: Arc<Mutex<Vec<String>>>,
    /// Shutdown signal checked by the nonblocking accept loop.
    stop: Arc<AtomicBool>,
    /// The server thread, joined when the test leaves scope.
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// Serve responses selected by request path, with bounded socket operations.
    fn new(handler: impl Fn(&str) -> Vec<u8> + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let server_requests = requests.clone();
        let server_stop = stop.clone();
        let thread = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !server_stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => return,
                };
                // Windows and BSD can inherit the listener's nonblocking mode;
                // request parsing needs blocking reads with the timeouts below.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut chunk = [0; 1_024];
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Ok(0) | Err(_) => break,
                        Ok(count) => request.extend_from_slice(&chunk[..count]),
                    }
                    if request.len() > 16 * 1_024 {
                        break;
                    }
                }
                // A closed, timed-out, or oversized request must never dispatch
                // a partial target to the handler as though its headers were complete.
                if request.len() > 16 * 1_024
                    || !request.windows(4).any(|bytes| bytes == b"\r\n\r\n")
                {
                    continue;
                }
                let text = String::from_utf8_lossy(&request);
                let path = text.split_whitespace().nth(1).unwrap_or("");
                server_requests.lock().unwrap().push(path.to_owned());
                // A client enforcing its input limit may close before consuming
                // the response, which is an expected outcome for these tests.
                let _ = stream.write_all(&handler(path));
            }
        });
        Self {
            origin,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    /// Return the complete request inventory after synchronous discovery ends.
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Construct a complete response that closes the connection after its body.
fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

/// Use short local timeouts and bypass environment proxy configuration.
fn client(status_as_error: bool) -> Client {
    Client::with_agent(
        transport::Agent::config_builder()
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(3)))
            .http_status_as_error(status_as_error)
            .build()
            .new_agent(),
    )
}

/// A partial request followed by EOF must not be treated as a complete request.
#[test]
fn incomplete_request_headers_are_not_dispatched() {
    let server = Server::new(|_| response("200 OK", "", b"unexpected dispatch"));
    let mut stream = TcpStream::connect(server.origin.strip_prefix("http://").unwrap()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .write_all(b"GET /partial HTTP/1.1\r\nHost: localhost\r\n")
        .unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    let mut received = Vec::new();
    stream.read_to_end(&mut received).unwrap();
    assert!(
        received.is_empty(),
        "incomplete request received a response: {received:?}"
    );
    assert!(server.requests().is_empty());
}

/// Discovery fetches the deployment and application manifests without payloads.
#[test]
fn discovery_fetches_exactly_two_manifests() {
    let server = Server::new(|path| match path {
        "/live/MTGO.application" => response("200 OK", "", DEPLOYMENT.as_bytes()),
        "/live/version/MTGO.exe.manifest" => response("200 OK", "", APPLICATION.as_bytes()),
        _ => response("404 Not Found", "", b"unexpected request"),
    });
    let application = client(true)
        .discover(&format!("{}/live/MTGO.application", server.origin))
        .unwrap();
    assert_eq!(application.resources().len(), 1);
    assert_eq!(
        application.resources()[0].url.as_str(),
        format!("{}/live/version/CardDataSource/Cards.xml", server.origin)
    );
    assert_eq!(
        server.requests(),
        ["/live/MTGO.application", "/live/version/MTGO.exe.manifest"]
    );
}

/// Both manifests resolve relative references against their final response URL.
#[test]
fn discovery_uses_final_urls_after_deployment_and_application_redirects() {
    let server = Server::new(|path| match path {
        "/entry" => response(
            "302 Found",
            "Location: /releases/live/MTGO.application\r\n",
            b"",
        ),
        "/releases/live/MTGO.application" => response("200 OK", "", DEPLOYMENT.as_bytes()),
        "/releases/live/version/MTGO.exe.manifest" => response(
            "302 Found",
            "Location: /cdn/build/MTGO.exe.manifest\r\n",
            b"",
        ),
        "/cdn/build/MTGO.exe.manifest" => response("200 OK", "", APPLICATION.as_bytes()),
        _ => response("404 Not Found", "", b"unexpected request"),
    });
    let application = client(true)
        .discover(&format!("{}/entry", server.origin))
        .unwrap_or_else(|error| panic!("{error:?}; requests: {:?}", server.requests()));
    assert_eq!(
        application.deployment().url().as_str(),
        format!("{}/releases/live/MTGO.application", server.origin)
    );
    assert_eq!(
        application.url().as_str(),
        format!("{}/cdn/build/MTGO.exe.manifest", server.origin)
    );
    assert_eq!(
        application.resources()[0].url.as_str(),
        format!("{}/cdn/build/CardDataSource/Cards.xml", server.origin)
    );
    assert_eq!(
        server.requests(),
        [
            "/entry",
            "/releases/live/MTGO.application",
            "/releases/live/version/MTGO.exe.manifest",
            "/cdn/build/MTGO.exe.manifest"
        ]
    );
}

/// HTTP failures cannot become valid manifests under permissive agent settings.
#[test]
fn non_success_status_is_rejected_with_or_without_transport_status_errors() {
    for status_as_error in [true, false] {
        let server = Server::new(|_| response("404 Not Found", "", DEPLOYMENT.as_bytes()));
        let result =
            client(status_as_error).discover(&format!("{}/MTGO.application", server.origin));
        assert!(matches!(
            result,
            Err(Error::Http(error)) if matches!(*error, transport::Error::StatusCode(404))
        ));
        assert_eq!(server.requests(), ["/MTGO.application"]);
    }
}

/// Streaming responses remain subject to the parser's explicit byte budget.
#[test]
fn chunked_response_is_bounded_without_a_content_length() {
    let server = Server::new(|_| {
        let mut response =
            b"HTTP/1.1 200 OK\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n400\r\n"
                .to_vec();
        response.extend_from_slice(&[b'x'; 1_024]);
        response.extend_from_slice(b"\r\n0\r\n\r\n");
        response
    });
    let result = client(true)
        .with_parse_options(clickonce::ParseOptions {
            max_input_bytes: 64,
            max_nodes: 250_000,
        })
        .discover(&format!("{}/MTGO.application", server.origin));
    assert!(matches!(
        result,
        Err(Error::Manifest(clickonce::Error::InputTooLarge {
            limit: 64
        }))
    ));
    assert_eq!(server.requests(), ["/MTGO.application"]);
}

/// Successful HTTP delivery still reports malformed manifest XML as a parse error.
#[test]
fn malformed_manifest_response_is_reported_without_a_second_request() {
    let server = Server::new(|_| response("200 OK", "", b"<assembly>"));
    let result = client(true).discover(&format!("{}/MTGO.application", server.origin));
    assert!(matches!(result, Err(Error::Manifest(_))));
    assert_eq!(server.requests(), ["/MTGO.application"]);
}

/// Invalid HTTP framing returns a transport error instead of attempting XML parsing.
#[test]
fn malformed_http_response_is_reported_as_transport_failure() {
    let server = Server::new(|_| b"not an HTTP response\r\n\r\n".to_vec());
    let result = client(true).discover(&format!("{}/MTGO.application", server.origin));
    assert!(matches!(result, Err(Error::Http(_))));
    assert_eq!(server.requests(), ["/MTGO.application"]);
}

/// Complete synthetic component inventory, with unrelated and index files included.
const CARD_RESOURCES: &[&str] = &[
    "CardDataSource/client_ONE.xml",
    "CardDataSource/client_ONE_DO.xml",
    "CardDataSource/client_TWO.xml",
    "CardDataSource/CARDNAME_STRING.xml",
    "CardDataSource/REAL_ORACLETEXT_STRING.xml",
    "CardDataSource/ARTIST_NAME_STRING.xml",
    "CardDataSource/MTGOV3CardData.xml",
    "CardDataSource/MTGOV3CardData.dtd",
    "legality.xml",
];

/// A base card whose name and rules text require separate lookup tables.
const BASE_SET: &str = r#"<CardSet id="ONE">
  <DigitalObject DigitalObjectCatalogID="DOC_100">
    <CARDNAME_STRING id="card-name" /><REAL_ORACLETEXT_STRING id="card-rules" />
    <DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD" />
  </DigitalObject>
</CardSet>"#;

/// A second fragment containing an inherited foil and a noncard booster object.
const OBJECT_SET: &str = r#"<CardSet id="ONE">
  <DigitalObject DigitalObjectCatalogID="DOC_101"><CLONE_ID value="100" /><IS_FOIL /></DigitalObject>
  <DigitalObject DigitalObjectCatalogID="DOC_102">
    <CARDNAME_STRING id="booster-name" /><DIGITAL_OBJECT_TYPE_CODE_STRING value="BSTR" />
  </DigitalObject>
</CardSet>"#;

/// An unrelated set that selected-set loading must leave unfetched.
const OTHER_SET: &str = r#"<CardSet id="TWO">
  <DigitalObject DigitalObjectCatalogID="DOC_200">
    <CARDNAME_STRING value="Other card" /><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD" />
  </DigitalObject>
</CardSet>"#;

/// Shared names are referenced by opaque identifiers rather than row positions.
const CARD_NAMES: &str = r#"<CARDNAME_STRING_ITEMS>
  <CARDNAME_STRING_ITEM id="booster-name">Example Booster</CARDNAME_STRING_ITEM>
  <CARDNAME_STRING_ITEM id="card-name">Example Card</CARDNAME_STRING_ITEM>
</CARDNAME_STRING_ITEMS>"#;

/// Rules text preserves decoded XML text during loading and clone inheritance.
const CARD_RULES: &str = r#"<REAL_ORACLETEXT_STRING_ITEMS>
  <REAL_ORACLETEXT_STRING_ITEM id="card-rules">Draw &amp; discard.</REAL_ORACLETEXT_STRING_ITEM>
</REAL_ORACLETEXT_STRING_ITEMS>"#;

/// An unused lookup proves selected loading follows actual field references.
const UNUSED_LOOKUP: &str = r#"<ARTIST_NAME_STRING_ITEMS><ARTIST_NAME_STRING_ITEM id="artist">Artist</ARTIST_NAME_STRING_ITEM></ARTIST_NAME_STRING_ITEMS>"#;

/// Construct an application snapshot locally so loader request counts exclude discovery.
fn card_application(origin: &str, paths: &[&str], mapped: bool) -> Application {
    let deployment = DEPLOYMENT.replace(
        "<deployment install=\"true\">",
        &format!("<deployment install=\"true\" mapFileExtensions=\"{mapped}\">"),
    );
    let mut files = String::new();
    for path in paths {
        // Append each known fixture path without allocating an intermediate string.
        files.push_str("<file name=\"");
        files.push_str(path);
        files.push_str("\" />");
    }
    let manifest = format!(
        r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
        <assemblyIdentity name="MTGO.exe" version="1.2.3.4" /><application />{files}</assembly>"#
    );
    Deployment::from_bytes(
        &format!("{origin}/live/MTGO.application"),
        deployment.as_bytes(),
    )
    .unwrap()
    .with_application_bytes(manifest.as_bytes())
    .unwrap()
}

/// Return known component data while making all unrelated requests fail visibly.
fn card_response(path: &str) -> Vec<u8> {
    let filename = path
        .strip_prefix("/live/version/CardDataSource/")
        .unwrap_or_default();
    let filename = filename.strip_suffix(".deploy").unwrap_or(filename);
    let body = match filename {
        "client_ONE.xml" => BASE_SET,
        "client_ONE_DO.xml" => OBJECT_SET,
        "client_TWO.xml" => OTHER_SET,
        "CARDNAME_STRING.xml" => CARD_NAMES,
        "REAL_ORACLETEXT_STRING.xml" => CARD_RULES,
        "ARTIST_NAME_STRING.xml" => UNUSED_LOOKUP,
        _ => return response("404 Not Found", "", b"unrequested component"),
    };
    response("200 OK", "", body.as_bytes())
}

/// Set selection loads both fragments and their lookups, including mapped payloads.
#[test]
fn selected_card_sets_load_fragments_and_referenced_tables_only() {
    for mapped in [false, true] {
        let server = Server::new(card_response);
        let application = card_application(&server.origin, CARD_RESOURCES, mapped);
        let database = client(true)
            .load_card_sets(&application, ["one", "ONE"])
            .unwrap();
        assert_eq!(database.len(), 3);
        assert_eq!(database.sets().count(), 2);
        assert_eq!(database.lookups().count(), 2);
        let base = database.card(100).unwrap();
        let foil = database.card(101).unwrap();
        assert_eq!(base.name(), Some("Example Card"));
        assert_eq!(foil.name(), base.name());
        assert_eq!(foil.rules_text(), Some("Draw & discard."));
        assert!(!base.is_foil().unwrap());
        assert!(foil.is_foil().unwrap());
        assert_eq!(
            database.object(102).unwrap().name(),
            Some("Example Booster")
        );
        assert!(matches!(
            database.card(102),
            Err(carddata::Error::NotCard(102))
        ));
        let suffix = if mapped { ".deploy" } else { "" };
        let expected: Vec<_> = [
            "client_ONE.xml",
            "client_ONE_DO.xml",
            "CARDNAME_STRING.xml",
            "REAL_ORACLETEXT_STRING.xml",
        ]
        .iter()
        .map(|filename| format!("/live/version/CardDataSource/{filename}{suffix}"))
        .collect();
        assert_eq!(server.requests(), expected);
    }
}

/// References inside nested groups request their own tables during selective loading.
#[test]
fn selected_loading_follows_references_inside_nested_property_groups() {
    let server = Server::new(|path| {
        let body = match path {
            "/live/version/CardDataSource/client_ONE.xml" => {
                r#"<CardSet id="ONE">
              <DigitalObject DigitalObjectCatalogID="DOC_7">
                <CARDNAME_STRING value="Nested card" /><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD" />
                <FLIPPED_ATTRIBUTES_LIST><FUTURE_GROUP><NESTED_TEXT id="text" /></FUTURE_GROUP></FLIPPED_ATTRIBUTES_LIST>
              </DigitalObject></CardSet>"#
            }
            "/live/version/CardDataSource/NESTED_TEXT.xml" => {
                r#"<NESTED_TEXT_ITEMS><NESTED_TEXT_ITEM id="text">Resolved face</NESTED_TEXT_ITEM></NESTED_TEXT_ITEMS>"#
            }
            _ => return response("404 Not Found", "", b"unexpected request"),
        };
        response("200 OK", "", body.as_bytes())
    });
    let application = card_application(
        &server.origin,
        &[
            "CardDataSource/client_ONE.xml",
            "CardDataSource/NESTED_TEXT.xml",
            "CardDataSource/ARTIST_NAME_STRING.xml",
        ],
        false,
    );
    let database = client(true).load_card_sets(&application, ["ONE"]).unwrap();
    let card = database.card(7).unwrap();
    let Some(carddata::ResolvedValue::Fields(face)) = card.field_ref("FLIPPED_ATTRIBUTES_LIST")
    else {
        panic!("expected a face group");
    };
    let Some(carddata::ResolvedValue::Fields(future)) = face.get("FUTURE_GROUP") else {
        panic!("expected a nested group");
    };
    assert_eq!(
        future.get("NESTED_TEXT"),
        Some(&carddata::ResolvedValue::Text("Resolved face"))
    );
    assert_eq!(database.lookups().count(), 1);
    assert_eq!(
        server.requests(),
        [
            "/live/version/CardDataSource/client_ONE.xml",
            "/live/version/CardDataSource/NESTED_TEXT.xml",
        ]
    );
}

/// Full database loading requests every component while skipping the master and DTD.
#[test]
fn full_card_database_loads_all_components_without_expanding_master_index() {
    let server = Server::new(card_response);
    let application = card_application(&server.origin, CARD_RESOURCES, false);
    let database = client(true).load_card_database(&application).unwrap();
    assert_eq!(database.len(), 4);
    assert_eq!(database.sets().count(), 3);
    assert_eq!(database.lookups().count(), 3);
    assert_eq!(database.card(200).unwrap().name(), Some("Other card"));
    assert_eq!(
        server.requests(),
        [
            "/live/version/CardDataSource/client_ONE.xml",
            "/live/version/CardDataSource/client_ONE_DO.xml",
            "/live/version/CardDataSource/client_TWO.xml",
            "/live/version/CardDataSource/CARDNAME_STRING.xml",
            "/live/version/CardDataSource/REAL_ORACLETEXT_STRING.xml",
            "/live/version/CardDataSource/ARTIST_NAME_STRING.xml",
        ]
    );
}

/// All requested set codes are checked before any network operation begins.
#[test]
fn absent_and_empty_card_set_selections_make_no_requests() {
    let server = Server::new(card_response);
    let application = card_application(&server.origin, CARD_RESOURCES, false);
    assert!(matches!(
        client(true).load_card_sets(&application, ["ONE", "ZZMISSING"]),
        Err(Error::MissingCardDataResource(code)) if code == "ZZMISSING"
    ));
    assert!(matches!(
        client(true).load_card_sets(&application, std::iter::empty::<&str>()),
        Err(Error::CardData(carddata::Error::InvalidData(_)))
    ));
    assert!(server.requests().is_empty());
}

/// Set and lookup payload identities must match their declared component filenames.
#[test]
fn card_data_payload_identity_mismatches_are_rejected() {
    for mismatch_lookup in [false, true] {
        let server = Server::new(move |path| {
            let mismatched = if mismatch_lookup {
                path.ends_with("CARDNAME_STRING.xml")
            } else {
                path.ends_with("client_ONE.xml")
            };
            if mismatched {
                let body: &[u8] = if mismatch_lookup {
                    b"<WRONG_ITEMS />"
                } else {
                    br#"<CardSet id="WRONG" />"#
                };
                response("200 OK", "", body)
            } else {
                card_response(path)
            }
        });
        let application = card_application(&server.origin, CARD_RESOURCES, false);
        let result = client(true).load_card_sets(&application, ["ONE"]);
        let expected_path = if mismatch_lookup {
            "CardDataSource/CARDNAME_STRING.xml"
        } else {
            "CardDataSource/client_ONE.xml"
        };
        assert!(
            matches!(
                &result,
                Err(Error::CardData(carddata::Error::InvalidData(message)))
                    if message.contains(expected_path)
            ),
            "{result:?}; requests: {:?}",
            server.requests()
        );
    }
}

/// Card-data limits apply to streamed responses independently of manifest limits.
#[test]
fn card_data_streaming_response_obeys_its_configured_byte_budget() {
    let server = Server::new(|_| {
        let mut response =
            b"HTTP/1.1 200 OK\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n400\r\n"
                .to_vec();
        response.extend_from_slice(&[b'x'; 1_024]);
        response.extend_from_slice(b"\r\n0\r\n\r\n");
        response
    });
    let application = card_application(&server.origin, &["CardDataSource/client_ONE.xml"], false);
    let result = client(true)
        .with_card_data_options(carddata::ParseOptions {
            max_input_bytes: 64,
            max_nodes: 250_000,
        })
        .load_card_sets(&application, ["ONE"]);
    assert!(matches!(
        result,
        Err(Error::CardData(carddata::Error::InputTooLarge {
            limit: 64
        }))
    ));
    assert_eq!(
        server.requests(),
        ["/live/version/CardDataSource/client_ONE.xml"]
    );
}

/// HTTP status failures remain transport errors during explicit payload loading.
#[test]
fn card_data_http_failure_is_rejected_with_a_permissive_agent() {
    let server = Server::new(|_| response("404 Not Found", "", BASE_SET.as_bytes()));
    let application = card_application(&server.origin, &["CardDataSource/client_ONE.xml"], false);
    let result = client(false).load_card_sets(&application, ["ONE"]);
    assert!(matches!(
        result,
        Err(Error::Http(error)) if matches!(*error, transport::Error::StatusCode(404))
    ));
    assert_eq!(
        server.requests(),
        ["/live/version/CardDataSource/client_ONE.xml"]
    );
}

/// Missing component files and unloaded clone bases preserve distinct typed errors.
#[test]
fn card_data_missing_lookup_and_clone_base_report_required_resources() {
    let server = Server::new(card_response);
    let application = card_application(&server.origin, &["CardDataSource/client_ONE.xml"], false);
    let result = client(true).load_card_sets(&application, ["ONE"]);
    assert!(
        matches!(
            &result,
            Err(Error::MissingCardDataResource(path)) if path == "CardDataSource/CARDNAME_STRING.xml"
        ),
        "{result:?}; requests: {:?}",
        server.requests()
    );
    assert_eq!(
        server.requests(),
        ["/live/version/CardDataSource/client_ONE.xml"]
    );

    let server = Server::new(|_| {
        response(
            "200 OK",
            "",
            br#"<CardSet id="ONE"><DigitalObject DigitalObjectCatalogID="DOC_101"><CLONE_ID value="100" /></DigitalObject></CardSet>"#,
        )
    });
    let application =
        card_application(&server.origin, &["CardDataSource/client_ONE_DO.xml"], false);
    let result = client(true).load_card_sets(&application, ["ONE"]);
    assert!(matches!(
        result,
        Err(Error::CardData(carddata::Error::MissingObject(100)))
    ));
    assert_eq!(
        server.requests(),
        ["/live/version/CardDataSource/client_ONE_DO.xml"]
    );
}

/// Relative artwork roots use the final configuration URL, including mapped payloads.
#[test]
fn artwork_configuration_uses_its_final_response_url() {
    for mapped in [false, true] {
        let server = Server::new(|path| match path {
            "/live/version/MTGO.exe.config" | "/live/version/MTGO.exe.config.deploy" => response(
                "302 Found",
                "Location: /cdn/settings/MTGO.exe.config\r\n",
                b"",
            ),
            "/cdn/settings/MTGO.exe.config" => response(
                "200 OK",
                "",
                br#"<configuration><downloadSource>
                    <artRoot value="../Graphics/" />
                    <cardArtSubPath value="Cards/Pics" />
                </downloadSource></configuration>"#,
            ),
            _ => response("404 Not Found", "", b"unexpected artwork request"),
        });
        let application = card_application(&server.origin, &["MTGO.exe.config"], mapped);
        let resolver = client(true).load_artwork(&application).unwrap();
        assert_eq!(
            resolver.base_url().as_str(),
            format!("{}/cdn/Graphics/Cards/Pics/", server.origin)
        );
        let suffix = if mapped { ".deploy" } else { "" };
        assert_eq!(
            server.requests(),
            [
                format!("/live/version/MTGO.exe.config{suffix}"),
                "/cdn/settings/MTGO.exe.config".to_owned(),
            ]
        );
    }
}

/// Missing resources make no request and valid-looking HTTP error bodies remain errors.
#[test]
fn artwork_configuration_missing_resource_and_http_status_are_rejected() {
    let server = Server::new(|_| {
        response(
            "404 Not Found",
            "",
            br#"<configuration><downloadSource>
                <artRoot value="https://art.example.test/Graphics/" />
                <cardArtSubPath value="Cards/Pics" />
            </downloadSource></configuration>"#,
        )
    });
    let missing = card_application(&server.origin, &["unrelated.config"], false);
    assert!(matches!(
        client(false).load_artwork(&missing),
        Err(Error::MissingResource(path)) if path == "MTGO.exe.config"
    ));
    assert!(server.requests().is_empty());

    let application = card_application(&server.origin, &["MTGO.exe.config"], false);
    assert!(matches!(
        client(false).load_artwork(&application),
        Err(Error::Http(error)) if matches!(*error, transport::Error::StatusCode(404))
    ));
    assert_eq!(server.requests(), ["/live/version/MTGO.exe.config"]);
}

/// Chunked configuration bodies cannot bypass the 16 MiB HTTP input budget.
#[test]
fn artwork_configuration_streaming_response_is_bounded() {
    let server = Server::new(|_| {
        let mut body = br#"<configuration><downloadSource>
            <artRoot value="https://art.example.test/Graphics/" />
            <cardArtSubPath value="Cards/Pics" />
        </downloadSource></configuration>"#
            .to_vec();
        // Without a byte limit this remains valid XML with trailing whitespace.
        body.resize(16 * 1024 * 1024 + 1, b' ');
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(&body);
        response.extend_from_slice(b"\r\n0\r\n\r\n");
        response
    });
    let application = card_application(&server.origin, &["MTGO.exe.config"], false);
    assert!(matches!(
        client(true).load_artwork(&application),
        Err(Error::Http(error))
            if matches!(*error, transport::Error::BodyExceedsLimit(limit) if limit == 16 * 1024 * 1024)
    ));
    assert_eq!(server.requests(), ["/live/version/MTGO.exe.config"]);
}

/// Auxiliary loaders use catalog mapping and follow redirects without fetching images.
#[test]
fn card_styles_load_from_the_declared_resource() {
    for mapped in [false, true] {
        // The response uses the real style file's declared Latin-1 encoding.
        let server = Server::new(|path| {
            match path {
            "/live/version/CardStyle.xml" | "/live/version/CardStyle.xml.deploy" => response(
                "302 Found", "Location: /styles.xml\r\n", b"",
            ),
            "/styles.xml" => response("200 OK", "", br#"<?xml version="1.0" encoding="ISO-8859-1"?>
                <AnimCache><AnimCardStyle Name="Child" CloneFrom="Base"/>
                <AnimCardStyle Name="Base"><Frame>Native Frame</Frame></AnimCardStyle></AnimCache>"#),
            _ => response("404 Not Found", "", b"unexpected resource"),
        }
        });
        let application =
            card_application(&server.origin, &["CardStyle.xml", "unrelated.xml"], mapped);
        let styles = client(true).load_card_styles(&application).unwrap();
        assert_eq!(
            styles.resolve("Child").unwrap().properties.frame.as_deref(),
            Some("Native Frame")
        );
        let suffix = if mapped { ".deploy" } else { "" };
        assert_eq!(
            server.requests(),
            [
                format!("/live/version/CardStyle.xml{suffix}"),
                "/styles.xml".to_owned()
            ]
        );
    }
}

/// Missing files, HTTP failures, and configured byte budgets fail before returning a catalog.
#[test]
fn card_style_loading_rejects_missing_resources_status_and_limits() {
    let server = Server::new(|_| response("200 OK", "", b"<AnimCache/>"));
    let missing = card_application(&server.origin, &["unrelated.xml"], false);
    assert!(
        matches!(client(true).load_card_styles(&missing), Err(Error::MissingResource(path)) if path == "CardStyle.xml")
    );
    assert!(server.requests().is_empty());
    let application = card_application(&server.origin, &["CardStyle.xml"], false);
    let limited = client(true).with_client_data_options(mtgoxml::cardstyle::ParseOptions {
        max_input_bytes: 5,
        max_nodes: 100,
    });
    assert!(matches!(
        limited.load_card_styles(&application),
        Err(Error::ClientData(
            mtgoxml::cardstyle::Error::InputTooLarge { limit: 5 }
        ))
    ));
    let failed = Server::new(|_| response("503 Service Unavailable", "", b"<AnimCache/>"));
    let application = card_application(&failed.origin, &["CardStyle.xml"], false);
    assert!(
        matches!(client(false).load_card_styles(&application), Err(Error::Http(error)) if matches!(*error, transport::Error::StatusCode(503)))
    );
}
