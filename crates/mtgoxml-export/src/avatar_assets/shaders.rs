//! Translate captured Direct3D bytecode into the GLSL used by the renderer.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use super::{Result, digest_bytes, digest_file};

/// Upper bound for one generated SPIR-V or GLSL program.
const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;

/// Translation tools resolved once for all avatars in one export.
pub(super) struct Compiler {
    /// Direct3D bytecode to SPIR-V compiler.
    vkd3d: PathBuf,
    /// SPIR-V to desktop GLSL translator.
    spirv_cross: PathBuf,
    /// Executable digests and version output retained in the source archive.
    provenance: Value,
}

impl Compiler {
    /// Locate both tools and capture their identities before translating a shader.
    ///
    /// # Errors
    /// Returns an error if a tool is missing, cannot report its version, or cannot be read.
    pub(super) fn new() -> Result<Self> {
        let vkd3d = executable("MTGOXML_VKD3D_COMPILER", "vkd3d-compiler")?;
        let spirv_cross = executable("MTGOXML_SPIRV_CROSS", "spirv-cross")?;
        let provenance = json!({
            "vkd3d": {"version": version(&vkd3d, "--version")?, "sha256": digest_file(&vkd3d)?},
            "spirv_cross": {"version": version(&spirv_cross, "--revision")?, "sha256": digest_file(&spirv_cross)?}
        });
        Ok(Self {
            vkd3d,
            spirv_cross,
            provenance,
        })
    }

    /// Return the translation tools' recorded identities.
    pub(super) fn provenance(&self) -> &Value {
        &self.provenance
    }

    /// Translate a captured shader and retain its generated programs and command arguments.
    ///
    /// # Errors
    /// Returns an error for a failed tool, missing or malformed generated output, or I/O failure.
    pub(super) fn translate(
        &self,
        bundle: &Path,
        input: &str,
        application_version: &str,
    ) -> Result<(String, Value)> {
        let name = Path::new(input)
            .file_stem()
            .and_then(OsStr::to_str)
            .ok_or("shader resource has no portable basename")?;
        // Resource identities distinguish equal basenames in separate client directories.
        let identity = digest_bytes(input.as_bytes())?;
        let spirv = format!("renderer/shaders/{name}-{}.spv", &identity[..12]);
        let glsl = format!("renderer/shaders/{name}-{}.glsl", &identity[..12]);
        fs::create_dir_all(bundle.join("renderer/shaders"))?;
        let bytecode_sha256 = digest_file(&bundle.join(input))?;
        let vkd3d_args = ["-x", "d3dbc", "-b", "spirv-binary", input, "-o", &spirv];
        run(&self.vkd3d, bundle, &vkd3d_args)?;
        let spirv_bytes = read_output(&bundle.join(&spirv))?;
        if spirv_bytes.len() < 20 || spirv_bytes[..4] != [0x03, 0x02, 0x23, 0x07] {
            return Err(format!("shader compiler produced invalid SPIR-V for {name}").into());
        }
        let cross_args = [
            spirv.as_str(),
            "--version",
            "330",
            "--no-es",
            "--flatten-ubo",
            "--force-temporary",
            "--output",
            glsl.as_str(),
        ];
        run(&self.spirv_cross, bundle, &cross_args)?;
        let generated = String::from_utf8(read_output(&bundle.join(&glsl))?)?;
        if !generated.starts_with("#version 330\n") || !generated.contains("void main(") {
            return Err(format!("SPIRV-Cross produced invalid GLSL for {name}").into());
        }
        let source = format!(
            "// Source: MTGO {application_version}, ShaderEffects.dll resource\n\
             // {input} (Direct3D pixel-shader bytecode).\n\
             // Bytecode SHA-256: {bytecode_sha256}\n\
             // vkd3d-compiler -x d3dbc -b spirv-binary INPUT.ps -o OUTPUT.spv\n\
             // spirv-cross INPUT.spv --version 330 --no-es --flatten-ubo --force-temporary --output OUTPUT.glsl\n\
             {generated}"
        );
        fs::write(bundle.join(&glsl), &source)?;
        let provenance = json!({
            "name": name,
            "bytecode": {"path": input, "sha256": bytecode_sha256},
            "spirv": {"path": spirv, "sha256": digest_bytes(&spirv_bytes)?},
            "glsl": {"path": glsl, "sha256": digest_bytes(source.as_bytes())?},
            "commands": [
                {"tool": "vkd3d-compiler", "arguments": vkd3d_args},
                {"tool": "spirv-cross", "arguments": cross_args}
            ]
        });
        Ok((source, provenance))
    }
}

/// Resolve an explicit tool override or find its executable in PATH.
fn executable(variable: &str, name: &str) -> Result<PathBuf> {
    let configured = std::env::var_os(variable).unwrap_or_else(|| name.into());
    if Path::new(&configured).is_file() {
        return Ok(fs::canonicalize(configured)?);
    }
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        // Include Windows' executable suffix without changing explicit filenames.
        let mut candidates = vec![directory.join(&configured)];
        if cfg!(windows) && Path::new(&configured).extension().is_none() {
            // Construct the extra candidate as an OS string so non-Unicode PATH entries survive.
            let mut filename: OsString = configured.clone();
            filename.push(".exe");
            candidates.push(directory.join(filename));
        }
        for candidate in candidates {
            if candidate.is_file() {
                return Ok(fs::canonicalize(candidate)?);
            }
        }
    }
    Err(format!("cannot find {name}; install the shader tools or set {variable}").into())
}

/// Capture a tool's version from either standard output or standard error.
fn version(program: &Path, argument: &str) -> Result<String> {
    let output = Command::new(program).arg(argument).output()?;
    if !output.status.success() {
        return Err(format!("cannot obtain shader tool version: {}", program.display()).into());
    }
    let text = format!(
        "{}{}",
        String::from_utf8(output.stdout)?,
        String::from_utf8(output.stderr)?
    );
    if text.trim().is_empty() {
        return Err(format!(
            "shader tool returned an empty version: {}",
            program.display()
        )
        .into());
    }
    Ok(text.trim().to_owned())
}

/// Run one translation stage in the capture bundle, preserving diagnostics on failure.
fn run(program: &Path, directory: &Path, arguments: &[impl AsRef<OsStr>]) -> Result<()> {
    let output = Command::new(program)
        .current_dir(directory)
        .args(arguments)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "shader translation failed ({}): {}{}",
            program.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

/// Read a complete bounded generated program before it is used or archived.
fn read_output(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_OUTPUT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_OUTPUT_BYTES {
        return Err(format!(
            "generated shader is empty or exceeds its size limit: {}",
            path.display()
        )
        .into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avatar_assets::Staging;

    /// Incomplete and oversized outputs cannot become render inputs.
    #[test]
    fn generated_outputs_must_be_present_nonempty_and_bounded() -> Result<()> {
        let stage = Staging::new(&std::env::temp_dir())?;
        let path = stage.path.join("shader.glsl");
        assert!(read_output(&path).is_err());
        fs::write(&path, [])?;
        assert!(read_output(&path).is_err());
        File::create(&path)?.set_len(MAX_OUTPUT_BYTES + 1)?;
        assert!(read_output(&path).is_err());
        fs::write(&path, b"generated shader")?;
        assert_eq!(read_output(&path)?, b"generated shader");
        Ok(())
    }

    /// A failing compiler cannot be mistaken for a successful conversion.
    #[test]
    fn failed_translation_retains_the_tool_diagnostic() -> Result<()> {
        let error = run(
            &std::env::current_exe()?,
            &std::env::current_dir()?,
            &["--not-a-valid-test-option"],
        )
        .unwrap_err();
        assert!(error.to_string().contains("shader translation failed"));
        Ok(())
    }

    /// Exercise both real tools against an explicitly supplied extracted client bundle.
    #[test]
    fn translates_captured_bytecode_when_requested() -> Result<()> {
        let Some(source) = std::env::var_os("MTGOXML_TEST_SHADER_BUNDLE") else {
            return Ok(());
        };
        let source = Path::new(&source);
        let stage = Staging::new(&std::env::temp_dir())?;
        let compiler = Compiler::new()?;
        for name in [
            "traditionalfoil",
            "assetfade",
            "etchedgold",
            "vclwinner",
            "liquidmetal",
            "inherentoverlayfoil",
            "trkenergize",
        ] {
            // Copy only the known source programs; translation creates the remaining files itself.
            let relative = format!("decompiled/ShaderEffects/effects/{name}/{name}.ps");
            let destination = stage.path.join(&relative);
            fs::create_dir_all(
                destination
                    .parent()
                    .ok_or("missing test shader directory")?,
            )?;
            fs::copy(source.join(&relative), destination)?;
            let (generated, provenance) =
                compiler.translate(&stage.path, &relative, "test capture")?;
            assert_eq!(
                generated,
                fs::read_to_string(
                    stage.path.join(
                        provenance["glsl"]["path"]
                            .as_str()
                            .ok_or("missing generated shader path")?
                    )
                )?
            );
            assert_eq!(
                provenance["glsl"]["sha256"],
                digest_bytes(generated.as_bytes())?
            );
        }
        Ok(())
    }
}
