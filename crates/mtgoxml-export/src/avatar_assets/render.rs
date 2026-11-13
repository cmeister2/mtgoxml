//! Native EGL/OpenGL rendering of captured MTGO avatar pixel shaders.
//!
//! Callers supply GLSL translated from the captured Direct3D bytecode through
//! vkd3d and SPIRV-Cross after verifying its source inputs.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::rc::Rc;

use glow::HasContext;
use image::RgbaImage;
use khronos_egl as egl;

use super::Result;
use super::animation::Program as Animation;

/// Mesa's platform identifier avoids requiring an X11 or Wayland session.
const PLATFORM_SURFACELESS_MESA: u32 = 0x31dd;
/// Full-screen triangle with the same logical texture/readback orientation.
const VERTEX_SHADER: &str = "#version 330
out vec2 v0;
void main() {
    vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
    v0 = p;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}";

/// EGL resources whose lifetime also pins the dynamically loaded library.
struct EglContext {
    /// The native library remains loaded until after all EGL handles are freed.
    api: egl::DynamicInstance<egl::EGL1_5>,
    /// Initialized Mesa display independent of a desktop session.
    display: egl::Display,
    /// Optional handle permits cleanup after partial initialization.
    context: Option<egl::Context>,
    /// A pbuffer makes the context usable without a window-system surface.
    surface: Option<egl::Surface>,
    /// Current EGL contexts must stay on their creating thread.
    _thread: PhantomData<Rc<()>>,
}

impl EglContext {
    /// Load EGL and create an OpenGL 3.3 core context on a tiny pbuffer.
    fn new() -> Result<Self> {
        // SAFETY: Only the platform EGL library is loaded. Its lifetime is owned
        // by this object; all function pointers remain valid until its drop.
        let api = unsafe { egl::DynamicInstance::<egl::EGL1_5>::load_required() }
            .map_err(|error| format!("cannot load EGL 1.5: {error}"))?;
        // SAFETY: The Mesa surfaceless platform specifies a null native display
        // and a terminated empty attribute list; no borrowed native object exists.
        let display = unsafe {
            api.get_platform_display(
                PLATFORM_SURFACELESS_MESA,
                std::ptr::null_mut(),
                &[egl::ATTRIB_NONE],
            )
        }
        .map_err(|error| format!("cannot obtain a surfaceless EGL display: {error}"))?;
        let mut state = Self {
            api,
            display,
            context: None,
            surface: None,
            _thread: PhantomData,
        };
        state
            .api
            .initialize(display)
            .map_err(|error| format!("cannot initialize EGL: {error}"))?;
        state
            .api
            .bind_api(egl::OPENGL_API)
            .map_err(|error| format!("cannot select desktop OpenGL: {error}"))?;
        let config = state
            .api
            .choose_first_config(
                display,
                &[
                    egl::SURFACE_TYPE,
                    egl::PBUFFER_BIT,
                    egl::RENDERABLE_TYPE,
                    egl::OPENGL_BIT,
                    egl::RED_SIZE,
                    8,
                    egl::GREEN_SIZE,
                    8,
                    egl::BLUE_SIZE,
                    8,
                    egl::ALPHA_SIZE,
                    8,
                    egl::NONE,
                ],
            )?
            .ok_or("no EGL RGBA8 pbuffer configuration is available")?;
        state.surface = Some(state.api.create_pbuffer_surface(
            display,
            config,
            &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE],
        )?);
        state.context = Some(state.api.create_context(
            display,
            config,
            None,
            &[
                egl::CONTEXT_MAJOR_VERSION,
                3,
                egl::CONTEXT_MINOR_VERSION,
                3,
                egl::CONTEXT_OPENGL_PROFILE_MASK,
                egl::CONTEXT_OPENGL_CORE_PROFILE_BIT,
                egl::NONE,
            ],
        )?);
        state.activate()?;
        Ok(state)
    }

    /// Make this object's context current before touching any GL resources.
    fn activate(&self) -> Result<()> {
        self.api
            .make_current(self.display, self.surface, self.surface, self.context)?;
        Ok(())
    }
}

impl Drop for EglContext {
    fn drop(&mut self) {
        let _ = self.api.make_current(self.display, None, None, None);
        if let Some(context) = self.context.take() {
            let _ = self.api.destroy_context(self.display, context);
        }
        if let Some(surface) = self.surface.take() {
            let _ = self.api.destroy_surface(self.display, surface);
        }
        let _ = self.api.terminate(self.display);
    }
}

/// A linked GL program and the locations needed for deterministic rendering.
struct Program {
    /// Linked program owned by the renderer's context.
    handle: glow::NativeProgram,
    /// Contiguous float4 array of the client's numeric constant registers.
    constants: Option<glow::NativeUniformLocation>,
    /// Register count prevents writing beyond the shader's uniform array.
    registers: usize,
    /// Source texture units and the corresponding combined sampler uniforms.
    samplers: Vec<(u32, glow::NativeUniformLocation)>,
}

/// An explicitly constructed, reusable headless renderer on the current thread.
pub(super) struct Renderer {
    /// GL entry points loaded while this object's EGL context was current.
    gl: glow::Context,
    /// Owns EGL handles and keeps the platform library alive.
    egl: EglContext,
    /// Dimensions of the derived regular-size preview.
    size: (u32, u32),
    /// Complete offscreen RGBA8 framebuffer.
    framebuffer: glow::NativeFramebuffer,
    /// Color attachment owned separately from the framebuffer name.
    color: glow::NativeTexture,
    /// Core profile requires a vertex array even when using gl_VertexID.
    vao: glow::NativeVertexArray,
    /// Programs are compiled once per distinct shader when first selected.
    programs: BTreeMap<String, Program>,
    /// Input textures are replaced for each avatar.
    textures: Vec<glow::NativeTexture>,
    /// A successful bind must precede frame rendering.
    selected: Option<String>,
}

impl Renderer {
    /// Create a headless renderer; no EGL context exists before this call.
    ///
    /// # Errors
    /// Returns an error for invalid dimensions or unavailable EGL/OpenGL support.
    pub(super) fn new(width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return Err("avatar rendering dimensions must be between 1 and 4096".into());
        }
        let egl = EglContext::new()?;
        // SAFETY: The EGL context is current and stays alive through Renderer.
        // Every returned address comes from that context's platform loader.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.api
                    .get_proc_address(name)
                    .map_or(std::ptr::null(), |function| {
                        function as *const () as *const c_void
                    })
            })
        };
        // SAFETY: The newly created context is current on this thread. All GL
        // objects below belong to it; texture dimensions are bounded above.
        let (framebuffer, color, vao) = unsafe {
            // Allocate the RGBA8 target, then configure deterministic pixel transfer and rendering state.
            let framebuffer = gl.create_framebuffer()?;
            let color = gl.create_texture()?;
            let vao = gl.create_vertex_array()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(color));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(color),
                0,
            );
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                return Err("avatar RGBA8 framebuffer is incomplete".into());
            }
            gl.viewport(0, 0, width as i32, height as i32);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.disable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            gl.disable(glow::DITHER);
            (framebuffer, color, vao)
        };
        Ok(Self {
            gl,
            egl,
            size: (width, height),
            framebuffer,
            color,
            vao,
            programs: BTreeMap::new(),
            textures: Vec::new(),
            selected: None,
        })
    }

    /// Describe the actual GL renderer and driver for output provenance.
    ///
    /// # Errors
    /// Returns an error if the EGL context cannot be made current.
    pub(super) fn description(&self) -> Result<(String, String)> {
        self.egl.activate()?;
        // SAFETY: GL string queries use the live current context.
        unsafe {
            Ok((
                self.gl.get_parameter_string(glow::RENDERER),
                self.gl.get_parameter_string(glow::VERSION),
            ))
        }
    }

    /// Select a generated shader and upload premultiplied textures to its exact samplers.
    ///
    /// # Errors
    /// Returns an error for unsupported effects, missing/extra samplers, invalid
    /// texture sizes, or GL shader compilation and upload failures.
    pub(super) fn bind(
        &mut self,
        name: &str,
        inputs: &BTreeMap<u32, RgbaImage>,
        source: &str,
    ) -> Result<()> {
        self.egl.activate()?;
        if !self.programs.contains_key(name) {
            self.programs
                .insert(name.to_owned(), compile_program(&self.gl, source)?);
        }
        let program = &self.programs[name];
        if program
            .samplers
            .iter()
            .any(|(unit, _)| !inputs.contains_key(unit))
        {
            return Err(format!("extracted texture bindings do not cover shader {name}").into());
        }
        // SAFETY: All handles belong to the live current context. Input byte
        // lengths come from RgbaImage; dimensions are checked before upload.
        unsafe {
            // Retire the previous avatar's textures before binding this program and uploading its inputs.
            for texture in self.textures.drain(..) {
                self.gl.delete_texture(texture);
            }
            self.gl.use_program(Some(program.handle));
            for (unit, location) in &program.samplers {
                // Premultiply each source image, upload it with clamped sampling, and connect its uniform.
                let image = &inputs[unit];
                if image.width() == 0
                    || image.height() == 0
                    || image.width() > 8192
                    || image.height() > 8192
                {
                    return Err("avatar input texture dimensions are invalid".into());
                }
                let mut pixels = image.as_raw().clone();
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    for index in 0..3 {
                        pixel[index] =
                            ((u16::from(pixel[index]) * u16::from(pixel[3]) + 127) / 255) as u8;
                    }
                }
                let texture = self.gl.create_texture()?;
                self.textures.push(texture);
                self.gl.active_texture(glow::TEXTURE0 + unit);
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                self.gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::LINEAR as i32,
                );
                self.gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::LINEAR as i32,
                );
                self.gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_S,
                    glow::CLAMP_TO_EDGE as i32,
                );
                self.gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_T,
                    glow::CLAMP_TO_EDGE as i32,
                );
                self.gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    image.width() as i32,
                    image.height() as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&pixels)),
                );
                self.gl.uniform_1_i32(Some(location), *unit as i32);
            }
        }
        check_gl(&self.gl, "avatar texture upload")?;
        self.selected = Some(name.to_owned());
        Ok(())
    }

    /// Sample a complete native timeline, coalescing consecutive unchanged constants.
    ///
    /// Every sampled frame retains its full interval so static portions of an
    /// animation do not require repeated GL readbacks or repeated pixel buffers.
    ///
    /// # Errors
    /// Returns an error for a zero frame interval, unsupported effects, invalid
    /// textures or shader interfaces, or GL rendering and readback failures.
    pub(super) fn sample_frames(
        &mut self,
        name: &str,
        inputs: &BTreeMap<u32, RgbaImage>,
        source: &str,
        animation: &Animation,
        frame_ms: u32,
    ) -> Result<Vec<(Vec<u8>, u32)>> {
        if frame_ms == 0 {
            return Err("avatar frame interval must be positive".into());
        }
        self.bind(name, inputs, source)?;
        let mut frames: Vec<(Vec<u8>, u32)> = Vec::new();
        let mut previous = None;
        let period = animation.period_ms();
        for millisecond in (0..period).step_by(frame_ms as usize) {
            // Retain the complete playback interval, including a partial final sample.
            let duration = frame_ms.min(period - millisecond);
            // Retain the complete display interval when the native constants have not changed.
            let values = animation.sample(f64::from(millisecond) / 1000.0)?;
            if previous.as_ref() == Some(&values) {
                if let Some((_, retained)) = frames.last_mut() {
                    *retained += duration;
                }
                continue;
            }
            frames.push((self.frame(&values)?, duration));
            previous = Some(values);
        }
        Ok(frames)
    }

    /// Render one RGB frame without palette quantization or a vertical flip.
    ///
    /// # Errors
    /// Returns an error if no avatar is bound or GL rendering/readback fails.
    pub(super) fn frame(&mut self, constants: &[[f32; 4]]) -> Result<Vec<u8>> {
        let name = self
            .selected
            .as_deref()
            .ok_or("bind avatar textures before rendering a frame")?;
        let program = &self.programs[name];
        self.egl.activate()?;
        let values = constants
            .get(..program.registers)
            .ok_or("extracted animation constants do not cover the shader interface")?
            .iter()
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        let mut output = vec![0; self.size.0 as usize * self.size.1 as usize * 3];
        // SAFETY: Every handle and uniform location belongs to the current
        // context. Uniform data has the exact register count; readback has the
        // required RGB byte length and PACK_ALIGNMENT is explicitly one.
        unsafe {
            // Select the target and program, upload the sampled constants, then draw and read back RGB.
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
            self.gl.bind_vertex_array(Some(self.vao));
            self.gl.use_program(Some(program.handle));
            self.gl
                .uniform_4_f32_slice(program.constants.as_ref(), &values);
            self.gl.draw_arrays(glow::TRIANGLES, 0, 3);
            self.gl.read_pixels(
                0,
                0,
                self.size.0 as i32,
                self.size.1 as i32,
                glow::RGB,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut output)),
            );
        }
        check_gl(&self.gl, "avatar frame readback")?;
        Ok(output)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        if self.egl.activate().is_err() {
            // Destroying the owning EGL context still releases its GL objects.
            return;
        }
        // SAFETY: The owning context is current; handles are deleted once and
        // the subsequent EglContext drop occurs only after this cleanup.
        unsafe {
            // Release avatar inputs and programs before deleting the shared drawing target.
            for texture in self.textures.drain(..) {
                self.gl.delete_texture(texture);
            }
            for program in self.programs.values() {
                self.gl.delete_program(program.handle);
            }
            self.gl.delete_vertex_array(self.vao);
            self.gl.delete_framebuffer(self.framebuffer);
            self.gl.delete_texture(self.color);
        }
    }
}

/// Compile one shader, deleting the failed object before returning diagnostics.
fn compile_shader(gl: &glow::Context, kind: u32, source: &str) -> Result<glow::NativeShader> {
    // SAFETY: This helper is called only with the owning EGL context current.
    unsafe {
        // Compile the supplied stage and return its handle only after checking the compiler result.
        let shader = gl.create_shader(kind)?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            // Preserve the compiler diagnostic before releasing the unusable shader object.
            let message = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            return Err(format!("avatar GLSL compilation failed: {message}").into());
        }
        Ok(shader)
    }
}

/// Link one exact shader interface and release temporary shader objects.
fn compile_program(gl: &glow::Context, source: &str) -> Result<Program> {
    let vertex = compile_shader(gl, glow::VERTEX_SHADER, VERTEX_SHADER)?;
    let fragment = match compile_shader(gl, glow::FRAGMENT_SHADER, source) {
        Ok(fragment) => fragment,
        Err(error) => {
            // SAFETY: The helper runs with the live owning context current.
            unsafe {
                gl.delete_shader(vertex);
            }
            return Err(error);
        }
    };
    // SAFETY: Compilation and linking occur on the live owning context, and
    // all object names below were created by it.
    unsafe {
        // Link both stages, release their temporary objects, and resolve the exact uniform interface.
        let handle = match gl.create_program() {
            Ok(handle) => handle,
            Err(error) => {
                // Program allocation failed, so neither compiled stage can be retained.
                gl.delete_shader(vertex);
                gl.delete_shader(fragment);
                return Err(error.into());
            }
        };
        gl.attach_shader(handle, vertex);
        gl.attach_shader(handle, fragment);
        gl.link_program(handle);
        gl.detach_shader(handle, vertex);
        gl.detach_shader(handle, fragment);
        gl.delete_shader(vertex);
        gl.delete_shader(fragment);
        if !gl.get_program_link_status(handle) {
            // Preserve the linker diagnostic before deleting the failed program.
            let message = gl.get_program_info_log(handle);
            gl.delete_program(handle);
            return Err(format!("avatar GLSL linking failed: {message}").into());
        }
        let uniforms = (|| -> Result<_> {
            // Reflect the linked program instead of assuming a historical register/sampler layout.
            let mut constants = None;
            let mut registers = 0;
            let mut samplers = Vec::new();
            for index in 0..gl.get_active_uniforms(handle) {
                // Resolve each active uniform's actual type, array length, and location.
                let uniform = gl
                    .get_active_uniform(handle, index)
                    .ok_or("cannot reflect a linked shader uniform")?;
                let location = gl
                    .get_uniform_location(handle, &uniform.name)
                    .ok_or("reflected shader uniform has no location")?;
                if uniform.utype == glow::FLOAT_VEC4
                    && uniform.name.starts_with("cb")
                    && uniform.name.ends_with("_struct[0]")
                {
                    // Accept one bounded float4 array and preserve its reflected size.
                    if constants.is_some() || !(1..=256).contains(&uniform.size) {
                        return Err("shader has an unsupported constant-register layout".into());
                    }
                    registers = uniform.size as usize;
                    constants = Some(location);
                } else if uniform.utype == glow::SAMPLER_2D {
                    // Recover the original texture register from the generated sampler name.
                    let suffix = uniform
                        .name
                        .strip_prefix("SPIRV_Cross_Combinedt")
                        .ok_or("shader has an unsupported sampler uniform")?;
                    let (texture, sampler) = suffix
                        .split_once('s')
                        .ok_or("shader sampler has no original register indices")?;
                    let texture: u32 = texture.parse()?;
                    let sampler: u32 = sampler.parse()?;
                    if texture > 15 || sampler > 15 {
                        return Err("shader sampler register exceeds supported limits".into());
                    }
                    samplers.push((texture, location));
                } else {
                    return Err(format!("unsupported shader uniform {}", uniform.name).into());
                }
            }
            if samplers.is_empty() {
                return Err("shader has no sampled image inputs".into());
            }
            Ok((constants, registers, samplers))
        })();
        match uniforms {
            Ok((constants, registers, samplers)) => Ok(Program {
                handle,
                constants,
                registers,
                samplers,
            }),
            Err(error) => {
                gl.delete_program(handle);
                Err(error)
            }
        }
    }
}

/// Convert GL errors to bounded, operation-specific diagnostics.
fn check_gl(gl: &glow::Context, operation: &str) -> Result<()> {
    // SAFETY: All callers have activated the owning EGL context.
    let error = unsafe { gl.get_error() };
    if error != glow::NO_ERROR {
        return Err(format!("{operation} failed with OpenGL error 0x{error:04x}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small independent blend exercises the renderer without copying MTGO shader code.
    const BLEND_SHADER: &str = "#version 330
uniform vec4 cb5_struct[5];
uniform sampler2D SPIRV_Cross_Combinedt0s0;
uniform sampler2D SPIRV_Cross_Combinedt1s1;
in vec2 v0;
out vec4 color;
void main() {
    color = mix(texture(SPIRV_Cross_Combinedt0s0, v0),
        texture(SPIRV_Cross_Combinedt1s1, v0), cb5_struct[4].x);
}";

    /// Exercise actual GPU texture uploads, premultiplied alpha and framebuffer row order.
    #[test]
    fn gpu_preserves_rgb_orientation_and_premultiplied_alpha() -> Result<()> {
        if std::env::var("MTGOXML_TEST_EGL").as_deref() != Ok("1") {
            return Ok(());
        }
        let mut renderer = Renderer::new(2, 2)?;
        let first = RgbaImage::from_raw(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 64, 240, 120, 20, 32,
            ],
        )
        .ok_or("invalid test image")?;
        let second = RgbaImage::from_raw(
            2,
            2,
            vec![
                10, 30, 50, 255, 70, 90, 110, 192, 130, 150, 170, 128, 190, 210, 230, 64,
            ],
        )
        .ok_or("invalid test image")?;
        let inputs = BTreeMap::from([(0, first.clone()), (1, second.clone())]);
        renderer.bind("blend", &inputs, BLEND_SHADER)?;
        for (progress, source) in [(0.0, first), (1.0, second)] {
            let mut values = [[0.0; 4]; 5];
            values[4][0] = progress;
            let pixels = renderer.frame(&values)?;
            let expected = source
                .pixels()
                .flat_map(|pixel| {
                    (0..3).map(move |index| {
                        ((u16::from(pixel[index]) * u16::from(pixel[3]) + 127) / 255) as u8
                    })
                })
                .collect::<Vec<_>>();
            assert_eq!(pixels, expected);
        }
        use super::super::animation::{Curve, Interpolation, Keyframe, Target, Timeline, Track};
        let animation = Animation::new(
            vec![[0.0; 4]; 5],
            vec![Track {
                target: Target::Scalar(4),
                timeline: Timeline {
                    curve: Curve::Keyframes(vec![
                        Keyframe {
                            seconds: 1.0,
                            value: 0.0,
                            interpolation: Interpolation::Linear,
                        },
                        Keyframe {
                            seconds: 1.5,
                            value: 1.0,
                            interpolation: Interpolation::Linear,
                        },
                        Keyframe {
                            seconds: 2.0,
                            value: 1.0,
                            interpolation: Interpolation::Linear,
                        },
                    ]),
                    duration: 2.25,
                    repeat: true,
                    auto_reverse: false,
                    acceleration: 0.0,
                    deceleration: 0.0,
                },
            }],
            "independent GPU fixture".into(),
        )?;
        assert!(
            renderer
                .sample_frames("blend", &inputs, BLEND_SHADER, &animation, 0)
                .is_err()
        );
        let frames = renderer.sample_frames("blend", &inputs, BLEND_SHADER, &animation, 500)?;
        assert_eq!(
            frames
                .iter()
                .map(|(_, duration)| *duration)
                .collect::<Vec<_>>(),
            [1500, 750]
        );
        Ok(())
    }

    /// Compile externally generated shaders against every supported native interface.
    #[test]
    fn gpu_generated_shaders_match_native_interfaces() -> Result<()> {
        let Some(directory) = std::env::var_os("MTGOXML_TEST_SHADER_DIR") else {
            return Ok(());
        };
        let directory = std::path::Path::new(&directory);
        let mut renderer = Renderer::new(2, 2)?;
        let image = RgbaImage::from_pixel(2, 2, image::Rgba([80, 160, 240, 255]));
        let mut programs = 0;
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("glsl") {
                continue;
            }
            let source = std::fs::read_to_string(&path)?;
            let textures = (0..16).map(|unit| (unit, image.clone())).collect();
            let key = path.to_str().ok_or("non-Unicode test shader path")?;
            renderer.bind(key, &textures, &source)?;
            assert_eq!(renderer.frame(&vec![[0.5; 4]; 256])?.len(), 12);
            programs += 1;
        }
        assert!(
            programs > 0,
            "provided shader directory contains no GLSL programs"
        );
        Ok(())
    }
}
