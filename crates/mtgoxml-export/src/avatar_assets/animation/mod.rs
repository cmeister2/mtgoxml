//! Structured .NET extraction and deterministic avatar preview animations.
//!
//! Captured client IL supplies image bindings, scalar defaults, and timelines.
//! Recognized WPF patterns are evaluated without loading a managed runtime.
//! Mouse interaction uses one reproducible virtual-pointer loop.

/// Shader resource, sampler, and dependency-property extraction.
pub(crate) mod bindings;
/// Complete captured effect definitions assembled for the exporter.
mod catalog;
/// Owned metadata and instructions decoded from captured .NET assemblies.
pub(crate) mod il;
/// Structural proof of the client's cached linear animation primitive.
mod interval;
/// Bounded recognition of WPF animation-construction methods.
pub(crate) mod patterns;
/// Declarative timelines and deterministic shader-constant sampling.
mod timeline;

pub(crate) use catalog::{Catalog, Effect};
pub(crate) use timeline::{
    Curve, Error, Interpolation, Keyframe, Program, Target, Timeline, Track,
};
