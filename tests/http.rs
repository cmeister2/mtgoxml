//! Local HTTP integration coverage for optional manifest discovery.

#![cfg(feature = "http")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use mtgoxml::{Client, Error, clickonce, transport};

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
                        Ok(0) | Err(_) => break,
                        Ok(count) => request.extend_from_slice(&chunk[..count]),
                    }
                    if request.len() > 16 * 1_024 {
                        break;
                    }
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
        .unwrap();
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
