//! Install a verified ILSpy executable and decompile captured client assemblies.

use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use mtgoxml::Url;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{MAX_RESOURCE_BYTES, Result, digest_bytes, download};

/// Extractor release and integrity information embedded with the exporter.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Config {
    /// ILSpy release, independent of the captured MTGO application version.
    version: String,
    /// Official package location for the native Linux executable.
    url: String,
    /// Expected SHA-256 of the complete NuGet archive.
    package_sha256: String,
    /// Expected SHA-256 of the native executable before invocation.
    executable_sha256: String,
    /// Exact archive entry selected without extracting unrelated package files.
    executable_path: String,
}

impl Config {
    /// Read the embedded settings without requiring runtime configuration files.
    pub(super) fn load() -> Result<Self> {
        Ok(toml::from_str(include_str!("ilspy.toml"))?)
    }

    /// Install either the official package's executable or an equally verified cache.
    pub(super) fn install(&self, directory: &Path, agent: &ureq::Agent) -> Result<Extractor> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return Err("avatar source extraction currently requires Linux x86_64".into());
        }
        let bytes = if let Some(supplied) = std::env::var_os("MTGOXML_ILSPY_PATH") {
            // Cached executables pass the same byte limit and binary digest check.
            let mut bytes = Vec::new();
            File::open(supplied)?
                .take(MAX_RESOURCE_BYTES + 1)
                .read_to_end(&mut bytes)?;
            bytes
        } else {
            let (package, _) = download(agent, &Url::parse(&self.url)?)?;
            self.package_executable(package)?
        };
        self.install_executable(directory, bytes)
    }

    /// Authenticate the complete package before reading its one expected executable.
    fn package_executable(&self, package: Vec<u8>) -> Result<Vec<u8>> {
        if digest_bytes(&package)? != self.package_sha256 {
            return Err(format!("ILSpy {} package digest mismatch", self.version).into());
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(package))?;
        let mut bytes = Vec::new();
        zip.by_name(&self.executable_path)?
            .take(MAX_RESOURCE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    /// Authenticate both download and cache paths before creating an executable file.
    fn install_executable(&self, directory: &Path, bytes: Vec<u8>) -> Result<Extractor> {
        if bytes.len() as u64 > MAX_RESOURCE_BYTES
            || digest_bytes(&bytes)? != self.executable_sha256
        {
            return Err(format!("ILSpy {} executable digest mismatch", self.version).into());
        }
        let path = directory.join("ilspycmd");
        fs::write(&path, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Extractor { path })
    }

    /// Retain the exact tool settings alongside captured assemblies and translations.
    pub(super) fn provenance(&self) -> Value {
        json!({
            "version":self.version,
            "url":self.url,
            "package_sha256":self.package_sha256,
            "executable_sha256":self.executable_sha256,
            "executable_path":self.executable_path,
        })
    }
}

/// Digest-verified executable owned by the current export's staging directory.
pub(super) struct Extractor {
    /// Installed executable, never a caller-supplied unverified command.
    path: PathBuf,
}

impl Extractor {
    /// Decompile one assembly and its embedded resources using captured references.
    pub(super) fn decompile(
        &self,
        assembly: &Path,
        references: &Path,
        output: &Path,
    ) -> Result<()> {
        let result = Command::new(&self.path)
            .args(["--disable-updatecheck", "-p", "-o"])
            .arg(output)
            .arg("-r")
            .arg(references)
            .arg(assembly)
            .output()?;
        if !result.status.success() {
            return Err(format!(
                "client source extraction failed: {}",
                String::from_utf8_lossy(&result.stderr)
            )
            .into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;

    /// A tiny package exercises the same trust checks without downloading ILSpy.
    fn package() -> Result<(Config, Vec<u8>, Vec<u8>)> {
        let executable = b"synthetic extractor".to_vec();
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "tools/any/linux-x64/ilspycmd",
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
        )?;
        zip.write_all(&executable)?;
        let package = zip.finish()?.into_inner();
        let config = Config {
            version: "test".into(),
            url: "https://example.test/extractor.nupkg".into(),
            package_sha256: digest_bytes(&package)?,
            executable_sha256: digest_bytes(&executable)?,
            executable_path: "tools/any/linux-x64/ilspycmd".into(),
        };
        Ok((config, package, executable))
    }

    /// A valid package and cached binary pass; corruption never creates an executable.
    #[test]
    fn package_and_cached_executables_require_their_own_digests() -> Result<()> {
        let (config, mut package, executable) = package()?;
        let staging = super::super::Staging::new(&std::env::temp_dir())?;
        let extracted = config.package_executable(package.clone())?;
        assert_eq!(extracted, executable);
        let tool = config.install_executable(&staging.path, extracted)?;
        assert_eq!(fs::read(&tool.path)?, executable);
        fs::remove_file(&tool.path)?;

        package[0] ^= 1;
        assert!(config.package_executable(package).is_err());
        assert!(
            config
                .install_executable(&staging.path, b"changed".to_vec())
                .is_err()
        );
        assert!(!staging.path.join("ilspycmd").exists());
        Ok(())
    }
}
