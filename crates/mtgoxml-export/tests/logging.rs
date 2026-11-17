//! CLI regressions for progress logs, filtering, and machine-readable stdout.

use std::process::{Command, Output};

use mockito::{Mock, Server, ServerGuard};
use mtgoxml::Deployment;
use tempfile::TempDir;

/// Captured publisher bytes retain the deployment's real application digest.
const DEPLOYMENT: &[u8] = include_bytes!("../../../fixtures/live/MTGO.application");
/// Matching application manifest makes a successful probe exercise digest verification.
const APPLICATION: &[u8] = include_bytes!("../../../fixtures/live/MTGO.exe.manifest");
/// Errors from fixture parsing, child processes, temporary files, and UTF-8 output.
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// One isolated probe supplies captured manifests through a local Mockito server.
struct Probe {
    /// Keeps the loopback server available until the child process exits.
    _server: ServerGuard,
    /// Private destination for the captured snapshot.
    directory: TempDir,
    /// Deployment endpoint passed to the real executable.
    deployment_url: String,
    /// Expected application endpoint derived from the captured deployment.
    application_url: String,
    /// Machine-readable tag that a successful probe prints.
    tag: String,
    /// Expected deployment request, including the requested HTTP status.
    deployment_request: Mock,
    /// Application requests occur only after a successful deployment response.
    application_request: Mock,
}

impl Probe {
    /// Serve the publisher fixtures, or reject the initial request with the supplied status.
    fn new(status: usize) -> Result<Self> {
        let mut server = Server::new();
        let deployment_url = format!("{}/MTGO.application", server.url());
        let deployment = Deployment::from_bytes(&deployment_url, DEPLOYMENT)?;
        let application_url = deployment.application_manifest_url().to_string();
        let application_path = deployment.application_manifest_url().path().to_owned();
        let application = deployment.with_application_bytes(APPLICATION)?;
        let tag = format!(
            "mtgo-data-{}",
            application.version().ok_or("missing fixture version")?
        );
        let deployment_request = server
            .mock("GET", "/MTGO.application")
            .with_status(status)
            .with_body(DEPLOYMENT)
            .expect(1)
            .create();
        let application_request = server
            .mock("GET", application_path.as_str())
            .with_status(200)
            .with_body(APPLICATION)
            .expect(usize::from(status == 200))
            .create();
        Ok(Self {
            _server: server,
            directory: TempDir::new()?,
            deployment_url,
            application_url,
            tag,
            deployment_request,
            application_request,
        })
    }

    /// Run the executable with per-process filters, without changing the test process environment.
    fn output(&self, filter: Option<&str>) -> Result<Output> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mtgoxml-export"));
        command
            .arg("probe")
            .arg(self.directory.path())
            .arg(&self.deployment_url)
            .env_remove("GITHUB_OUTPUT")
            .env_remove("RUST_LOG")
            .env_remove("RUST_LOG_STYLE");
        if let Some(filter) = filter {
            command.env("RUST_LOG", filter);
        }
        Ok(command.output()?)
    }

    /// Require the exact manifest requests and successful machine-readable output.
    fn assert_success(&self, output: &Output) {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, format!("{}\n", self.tag).as_bytes());
        assert!(self.directory.path().join("snapshot.json").is_file());
        self.deployment_request.assert();
        self.application_request.assert();
    }
}

/// Default INFO logs describe real progress on stderr without contaminating stdout.
#[test]
fn default_progress_uses_stderr_and_preserves_probe_stdout() -> Result<()> {
    let probe = Probe::new(200)?;
    let output = probe.output(None)?;
    probe.assert_success(&output);
    let stderr = std::str::from_utf8(&output.stderr)?;
    assert!(stderr.contains("INFO"), "{stderr}");
    assert!(
        stderr.contains("Downloading manifests: 0/2, started"),
        "{stderr}"
    );
    assert!(
        stderr.contains("Downloading manifests: 2/2, complete"),
        "{stderr}"
    );
    assert!(stderr.contains("Captured MTGO 3.4.159.4708"), "{stderr}");
    assert!(!stderr.contains("DEBUG"), "{stderr}");
    assert!(!stderr.contains(['\u{1b}', '\r']), "{stderr:?}");
    Ok(())
}

/// RUST_LOG controls request diagnostics and disables all progress output when set to off.
#[test]
fn rust_log_controls_request_diagnostics_and_silence() -> Result<()> {
    for filter in ["debug", "warn", "off"] {
        let probe = Probe::new(200)?;
        let output = probe.output(Some(filter))?;
        probe.assert_success(&output);
        let stderr = std::str::from_utf8(&output.stderr)?;
        if filter == "debug" {
            assert!(stderr.contains("DEBUG"), "{stderr}");
            assert!(
                stderr.contains(&format!("GET {}", probe.deployment_url)),
                "{stderr}"
            );
            assert!(
                stderr.contains(&format!("GET {}", probe.application_url)),
                "{stderr}"
            );
        } else {
            assert!(stderr.is_empty(), "{stderr}");
        }
    }
    Ok(())
}

/// A failed request reports ERROR and the completed count instead of false completion.
#[test]
fn failed_probe_reports_error_and_unfinished_progress() -> Result<()> {
    let probe = Probe::new(404)?;
    let output = probe.output(None)?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = std::str::from_utf8(&output.stderr)?;
    assert!(stderr.contains("ERROR"), "{stderr}");
    assert!(stderr.contains("404"), "{stderr}");
    assert!(
        stderr.contains("Downloading manifests: 0/2, stopped"),
        "{stderr}"
    );
    assert!(!stderr.contains("2/2"), "{stderr}");
    assert!(!probe.directory.path().join("snapshot.json").exists());
    probe.deployment_request.assert();
    probe.application_request.assert();
    Ok(())
}
