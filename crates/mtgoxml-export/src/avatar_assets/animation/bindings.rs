//! Recover shader properties and image assignments from captured managed IL.
//!
//! This bounded evaluator models dependency-property registration, image brushes,
//! and avatar bindings. It never loads or executes client assemblies. Unrecognized
//! calls, branches, or property values stop extraction instead of inventing a slot.

use std::collections::BTreeMap;

use super::super::Result;
use super::il::Assembly;
use machine::Machine;

/// Bounded declaration interpreter and supported WPF carrier operations.
mod machine;

/// The source image assigned to one shader sampler by the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// The avatar's ordinary rendered image.
    Input,
    /// Its first avatar-specific effect image.
    AvatarOne,
    /// Its additional avatar-specific effect image.
    AvatarTwo,
    /// An embedded image, relative to the preserved capture bundle.
    Shared(String),
}

/// A registered constant property, including aliases of a vector component.
#[derive(Debug, Clone)]
pub(crate) struct Constant {
    /// Float4 register associated with the dependency property.
    pub register: usize,
    /// First component written by this property or scalar alias.
    pub component: usize,
    /// Number of components supplied by the original property type.
    pub components: usize,
    /// Initial register contents after constructors and overrides, with WPF's one-filled padding.
    pub default: [f32; 4],
    /// Constructor update order; zero denotes an initially inactive property.
    pub priority: usize,
}

/// Captured-client shader interface and its original resource assignments.
#[derive(Debug, Clone)]
pub(crate) struct Bindings {
    /// Fully qualified managed effect class selected for the avatar.
    pub class: String,
    /// Original pixel-shader resource, relative to the preserved bundle.
    pub shader_resource: String,
    /// Image sources keyed by the actual declared sampler number.
    pub samplers: BTreeMap<u32, Source>,
    /// Constant properties keyed by their declaring type and field name.
    pub constants: BTreeMap<String, Constant>,
    /// Scalar dependency-property values used by animation constructors and branches.
    pub scalar_defaults: BTreeMap<String, f64>,
}

/// Extract an avatar's interface from its catalog discriminator and captured IL.
///
/// # Errors
/// Returns an error for absent declarations or unfamiliar client behavior.
pub(crate) fn extract(assemblies: &[Assembly], effect: u32) -> Result<Bindings> {
    Machine::new(assemblies, effect).extract()
}

#[cfg(test)]
mod tests;
