//! Assemble extracted shader bindings and supported timelines once per export.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Component, Path};

use serde_json::{Value, json};

use super::bindings::{self, Bindings};
use super::il::Assembly;
use super::{Program, Target, Timeline, Track, patterns};
use crate::Result;

/// A captured effect's shader inputs and complete deterministic preview timeline.
pub(crate) struct Effect {
    /// Resource and property bindings recovered from the original client IL.
    pub(crate) bindings: Bindings,
    /// Constants sampled by the renderer, including the standard pointer policy.
    pub(crate) animation: Program,
}

/// Each requested effect either has extracted behavior or an explicit explanation.
#[derive(Default)]
pub(crate) struct Catalog {
    /// Catalog discriminators retain their original values independently of client enums.
    effects: BTreeMap<u32, std::result::Result<Effect, String>>,
}

impl Catalog {
    /// Read the captured assemblies and extract each distinct requested effect.
    ///
    /// # Errors
    /// Returns an error for unreadable or malformed assemblies. Unsupported effect
    /// behavior is retained per effect so its sources can still be archived.
    pub(crate) fn load(bundle: &Path, effects: impl IntoIterator<Item = u32>) -> Result<Self> {
        let codes: BTreeSet<_> = effects.into_iter().collect();
        if codes.is_empty() {
            return Ok(Self::default());
        }
        let mut assemblies = Vec::new();
        for name in [
            "ShaderEffects.dll",
            "Core.dll",
            "WotC.MtGO.Client.Model.Core.dll",
        ] {
            let bytes = crate::read_bounded(File::open(bundle.join("original").join(name))?)?;
            assemblies.push(Assembly::parse(&bytes)?);
        }
        let mut catalog = Self::default();
        for code in codes {
            catalog.effects.insert(
                code,
                extract(&assemblies, code).map_err(|error| error.to_string()),
            );
        }
        Ok(catalog)
    }

    /// Return the extracted definition or the reason its client behavior is unsupported.
    ///
    /// # Errors
    /// Returns the captured extraction diagnostic, or an error if the effect was not requested.
    pub(crate) fn get(&self, code: u32) -> std::result::Result<&Effect, &str> {
        self.effects
            .get(&code)
            .ok_or("effect was not requested for extraction")?
            .as_ref()
            .map_err(String::as_str)
    }

    /// Retain the source classes, resources, and preview policy beside the captured inputs.
    pub(crate) fn provenance(&self) -> Value {
        let effects: Vec<_> = self
            .effects
            .iter()
            .map(|(code, result)| match result {
                Ok(effect) => json!({
                    "premium_effect":code,
                    "class":effect.bindings.class,
                    "shader":effect.bindings.shader_resource,
                    "period_ms":effect.animation.period_ms(),
                    "timing":effect.animation.timing()
                }),
                Err(reason) => json!({"premium_effect":code,"unsupported":reason}),
            })
            .collect();
        json!({"source":"managed_metadata_and_il","effects":effects})
    }
}

/// Combine one effect's actual declarations with the deterministic interaction policy.
fn extract(assemblies: &[Assembly], code: u32) -> Result<Effect> {
    let bindings = bindings::extract(assemblies, code)?;
    validate_resource(&bindings.shader_resource)?;
    for source in bindings.samplers.values() {
        if let bindings::Source::Shared(path) = source {
            validate_resource(path)?;
        }
    }
    let shader_assembly = assemblies.first().ok_or("missing shader assembly")?;
    let mut animations =
        patterns::animations(shader_assembly, &bindings.class, &bindings.scalar_defaults)?;
    // Apply the actual hover handler's property changes before replacing its
    // pointer motion with the standard loop, including any non-time tracks.
    if animations.mouse_loop {
        for stopped in &animations.hover_tracks {
            animations
                .tracks
                .retain(|track| track.property != stopped.property);
        }
        animations.tracks.append(&mut animations.hover_tracks);
    }
    let count = bindings
        .constants
        .values()
        .map(|constant| constant.register + 1)
        .max()
        .unwrap_or(0);
    if count > 256 {
        return Err("extracted constant register count exceeds renderer limits".into());
    }
    let mut defaults = vec![[0.0; 4]; count];
    let mut constants: Vec<_> = bindings.constants.values().collect();
    constants.sort_by_key(|constant| constant.priority);
    for constant in constants {
        if constant.component + constant.components > 4 {
            return Err("extracted property exceeds one float4 register".into());
        }
        defaults[constant.register] = constant.default;
    }
    let mut tracks = Vec::new();
    for track in animations.tracks {
        // Bind the recovered scalar timeline to its actual shader register.
        let constant = bindings
            .constants
            .get(&track.property)
            .ok_or_else(|| format!("animation targets an unbound property {}", track.property))?;
        if constant.components != 1 || constant.component != 0 {
            return Err(
                format!("scalar timeline targets vector property {}", track.property).into(),
            );
        }
        tracks.push(Track {
            target: Target::Scalar(constant.register),
            timeline: track.timeline,
        });
    }
    if animations.mouse_loop {
        for name in ["MouseXProperty", "MouseYProperty"] {
            // Resolve each virtual-pointer axis through the client's property callback.
            let suffix = format!("::{name}");
            let matching: Vec<_> = bindings
                .constants
                .iter()
                .filter(|(field, _)| field.ends_with(&suffix))
                .collect();
            if matching.len() != 1 {
                return Err(format!("standard pointer has no unambiguous {name} binding").into());
            }
            let constant = matching[0].1;
            tracks.push(Track {
                target: Target::Component {
                    register: constant.register,
                    component: constant.component,
                },
                timeline: Timeline::mouse_loop(),
            });
        }
    }
    let timing = if animations.mouse_loop && animations.hover_stops_time {
        "IL-extracted animation; standard 12s pointer loop; client hover time-stop; deterministic zero random phase/delay"
    } else if animations.mouse_loop {
        "IL-extracted animation; standard 12s pointer loop; deterministic zero random phase/delay"
    } else {
        "IL-extracted animation; deterministic zero random phase/delay; one-shot tracks retrigger at preview end"
    };
    let animation = Program::new(defaults, tracks, timing.into())?;
    if animation.registers() > 256 {
        return Err("extracted animation exceeds renderer register limits".into());
    }
    Ok(Effect {
        bindings,
        animation,
    })
}

/// Resource assignments must remain inside the decompiled capture directory.
fn validate_resource(path: &str) -> Result<()> {
    if !path.starts_with("decompiled/")
        || path.contains(['\\', ':', '\0'])
        || Path::new(path)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || path.split('/').any(str::is_empty)
    {
        return Err(format!("unsupported captured resource path {path}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avatar_assets::{self, Definition, Staging, render, shaders, texture};
    use std::fs;

    /// Exercise the complete extraction/render/codec path against an explicitly supplied capture.
    #[test]
    fn captured_effects_drive_verified_native_previews() -> Result<()> {
        let Some(bundle) = std::env::var_os("MTGOXML_TEST_AVATAR_BUNDLE") else {
            return Ok(());
        };
        let bundle = Path::new(&bundle);
        let avatars: Vec<Definition> =
            serde_json::from_reader(File::open(bundle.join("avatars.json"))?)?;
        let codes: BTreeSet<_> = avatars.iter().map(|avatar| avatar.premium_effect).collect();
        let catalog = Catalog::load(bundle, codes.iter().copied())?;
        for code in &codes {
            let effect = catalog
                .get(*code)
                .map_err(|reason| format!("effect {code}: {reason}"))?;
            assert!(bundle.join(&effect.bindings.shader_resource).is_file());
            println!(
                "Extracted effect {code}: {} ({}ms)",
                effect.bindings.class,
                effect.animation.period_ms()
            );
        }
        if std::env::var("MTGOXML_TEST_EGL").as_deref() != Ok("1") {
            return Ok(());
        }
        let staging = Staging::new(&std::env::temp_dir())?;
        let output = staging.path.join("bundle");
        fs::create_dir_all(output.join("webp"))?;
        let compiler = shaders::Compiler::new()?;
        let mut renderer = render::Renderer::new(avatar_assets::WIDTH, avatar_assets::HEIGHT)?;
        let mut programs = BTreeMap::new();
        let mut rendered = 0;
        for code in codes {
            let effect = catalog.get(code).map_err(str::to_owned)?;
            let mut available = None;
            for avatar in avatars
                .iter()
                .filter(|avatar| avatar.premium_effect == code)
            {
                let textures = texture::samplers(bundle, avatar, &effect.bindings.samplers)?;
                if textures.values().all(|texture| texture.path().is_file()) {
                    available = Some((avatar, textures));
                    break;
                }
            }
            let Some((avatar, textures)) = available else {
                continue;
            };
            let resource = &effect.bindings.shader_resource;
            if !programs.contains_key(resource) {
                let target = output.join(resource);
                fs::create_dir_all(target.parent().ok_or("missing shader output parent")?)?;
                fs::copy(bundle.join(resource), &target)?;
                let (source, _) = compiler.translate(&output, resource, "supplied capture")?;
                programs.insert(resource.clone(), source);
            }
            let row = avatar_assets::render_avatar(
                &output,
                avatar,
                effect,
                &mut renderer,
                &textures,
                &programs[resource],
            )?;
            assert_eq!(row["duration_ms"], effect.animation.period_ms());
            assert_eq!(row["pixel_timeline_exact"], true);
            println!(
                "Verified effect {code}: avatar {}, {} frames, {} bytes",
                avatar.id, row["encoded_frames"], row["size"]
            );
            rendered += 1;
        }
        assert!(rendered > 0, "capture has no available previews");
        Ok(())
    }
}
