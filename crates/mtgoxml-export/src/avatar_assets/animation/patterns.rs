//! Recognize scalar WPF animation construction from bounded, decoded IL.
//!
//! This interpreter implements the numeric and object-construction operations
//! used to build animation data. It never loads managed code into a runtime or
//! executes arbitrary methods. Unknown calls, instructions, exception handlers,
//! or excessive recursion are explicit unsupported-pattern errors.

use crate::Result;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use super::il::{Assembly, Constant, FieldRef, Instruction, Method, MethodRef, OpCode, Operand};
use super::{Curve, Error, Interpolation, Keyframe, Timeline};

/// Maximum decoded operations evaluated across nested animation methods.
const MAX_STEPS: usize = 100_000;
/// Maximum simultaneous managed helper calls.
const MAX_DEPTH: usize = 32;
/// Maximum stack or local-variable slots in a supported method.
const MAX_SLOTS: usize = 1024;
/// Maximum declarative keyframes emitted by one method.
const MAX_KEYFRAMES: usize = 4096;

/// A dependency property and the recovered scalar animation assigned to it.
#[derive(Debug, Clone)]
pub(crate) struct AnimationTrack {
    /// Original `owner::field` identity passed to WPF BeginAnimation.
    pub(crate) property: String,
    /// Extracted timeline, before deterministic preview interaction policy.
    pub(crate) timeline: Timeline,
}

/// Animation operations recovered from the effect's actual startup method.
#[derive(Debug, Clone)]
pub(crate) struct AnimationSet {
    /// Scalar dependency-property animations started by the client.
    pub(crate) tracks: Vec<AnimationTrack>,
    /// Captured stop animations used when the standard pointer policy represents hover.
    pub(crate) hover_tracks: Vec<AnimationTrack>,
    /// Native mouse-animation calls that should use the standard pointer loop.
    pub(crate) mouse_loop: bool,
    /// The class's hover handler reaches StopTimeAnimation through virtual dispatch.
    pub(crate) hover_stops_time: bool,
}

/// Extract startup animations with deterministic visibility, pointer, and random policy.
///
/// # Errors
/// Returns an unsupported-pattern error for unfamiliar IL or missing property
/// defaults, or an invalid-animation error for malformed extracted timelines.
pub(crate) fn animations(
    assembly: &Assembly,
    class: &str,
    defaults: &BTreeMap<String, f64>,
) -> Result<AnimationSet> {
    let mut evaluator = Evaluator::new(assembly, class, defaults);
    let method = evaluator.resolve(class, "StartAnimations", 1)?;
    evaluator.execute(&method, vec![Value::This, Value::Integer(0)], 0)?;
    let tracks = std::mem::take(&mut evaluator.tracks);
    if evaluator.mouse_loop {
        // Apply the client's hover event path before choosing which startup tracks to replace.
        evaluator.hover_phase = true;
        let hover = evaluator.resolve(class, "HandleMouseEnter", 0)?;
        evaluator.execute(&hover, vec![Value::This], 0)?;
    }
    Ok(AnimationSet {
        tracks,
        hover_tracks: evaluator.hover_tracks,
        mouse_loop: evaluator.mouse_loop,
        hover_stops_time: evaluator.hover_stops_time,
    })
}

/// Extract one animation-construction method without interpreting an effect instance.
///
/// # Errors
/// Returns an error for unsupported IL, invalid arguments, or a non-timeline result.
#[cfg(test)]
fn extract(assembly: &Assembly, owner: &str, method: &str, arguments: &[f64]) -> Result<Timeline> {
    let defaults = BTreeMap::new();
    let mut evaluator = Evaluator::new(assembly, owner, &defaults);
    let method = evaluator.resolve(owner, method, arguments.len())?;
    let mut arguments: Vec<_> = arguments.iter().copied().map(Value::Number).collect();
    if !method.is_static {
        arguments.insert(0, Value::This);
    }
    evaluator.execute(&method, arguments, 0)?.timeline()
}

/// Extract a static timeline from its initializer without executing unrelated static code.
///
/// # Errors
/// Returns an error for unsupported initialization, missing fields, or a non-timeline result.
#[cfg(test)]
fn extract_static(assembly: &Assembly, owner: &str, field: &str) -> Result<Timeline> {
    let defaults = BTreeMap::new();
    let mut evaluator = Evaluator::new(assembly, owner, &defaults);
    let ty = assembly
        .types
        .get(owner)
        .ok_or_else(|| unsupported(owner, None, "missing type"))?;
    let field = ty
        .fields
        .get(field)
        .ok_or_else(|| unsupported(owner, None, "missing field"))?;
    evaluator
        .static_value(
            &FieldRef {
                owner: owner.into(),
                name: field.name.clone(),
                ty: field.ty.clone(),
            },
            0,
        )?
        .timeline()
}

/// Pending timeline state, shared by duplicated IL object references.
#[derive(Debug, Clone)]
struct Builder {
    /// Declarative curve populated by constructors and keyframe additions.
    curve: Curve,
    /// Whether the range specifies From instead of using the target property's base value.
    explicit_from: bool,
    /// Whether the range specifies To instead of using the target property's base value.
    explicit_to: bool,
    /// Explicit duration; keyframe animations otherwise use their final key time.
    duration: Option<f64>,
    /// Whether RepeatBehavior.Forever was assigned.
    repeat: bool,
    /// Whether Timeline.AutoReverse was assigned.
    auto_reverse: bool,
    /// AccelerationRatio supplied by client code.
    acceleration: f64,
    /// DecelerationRatio supplied by client code.
    deceleration: f64,
}

impl Builder {
    /// Convert mutable construction state into a validated immutable timeline.
    fn finish(&self, origin: Option<f64>) -> Result<Timeline> {
        let duration = self.duration.unwrap_or_else(|| match &self.curve {
            Curve::Keyframes(frames) => frames.last().map_or(0.0, |frame| frame.seconds),
            Curve::Range { .. } => 1.0,
        });
        let mut curve = self.curve.clone();
        match &mut curve {
            Curve::Range { from, to } => {
                if !self.explicit_from {
                    *from = origin.ok_or_else(|| {
                        Error::Invalid("range requires its target property's origin".into())
                    })?;
                }
                if !self.explicit_to {
                    *to = origin.ok_or_else(|| {
                        Error::Invalid("range requires its target property's destination".into())
                    })?;
                }
            }
            Curve::Keyframes(frames) if frames.first().is_some_and(|frame| frame.seconds > 0.0) => {
                frames.insert(
                    0,
                    Keyframe {
                        seconds: 0.0,
                        value: origin.ok_or_else(|| {
                            Error::Invalid(
                                "keyframes require their target property's origin".into(),
                            )
                        })?,
                        interpolation: Interpolation::Linear,
                    },
                );
            }
            _ => {}
        }
        let timeline = Timeline {
            curve,
            duration,
            repeat: self.repeat,
            auto_reverse: self.auto_reverse,
            acceleration: self.acceleration,
            deceleration: self.deceleration,
        };
        timeline.validate()?;
        Ok(timeline)
    }
}

/// A keyframe keeps its easing object until collection insertion is complete.
#[derive(Debug, Clone)]
struct Frame {
    /// Time, value, and initial interpolation supplied by the constructor.
    frame: Keyframe,
    /// Optional SineEase object's mutable EasingMode.
    easing: Option<Rc<Cell<i32>>>,
}

/// Values permitted on the bounded animation-construction stack.
#[derive(Debug, Clone)]
enum Value {
    /// CLI int32 values retain integer arithmetic and comparison semantics.
    Integer(i32),
    /// CLI floating-point values retain their arithmetic category.
    Number(f64),
    /// Null references and void return values.
    Null,
    /// The effect instance; no client object is ever instantiated.
    This,
    /// A dependency-property field's original metadata identity.
    Property(String),
    /// A recognized animation under construction.
    Timeline(Rc<RefCell<Builder>>),
    /// The keyframe collection of the referenced timeline.
    Collection(Rc<RefCell<Builder>>),
    /// A scalar keyframe under construction.
    Frame(Rc<RefCell<Frame>>),
    /// A SineEase object's mode, without a WPF runtime object.
    Easing(Rc<Cell<i32>>),
    /// WPF's RepeatBehavior.Forever marker.
    Forever,
    /// Random is recognized solely to choose the documented zero phase/delay.
    Random,
}

impl Value {
    /// Require a finite numeric value at an arithmetic or configuration boundary.
    fn number(&self) -> Result<f64> {
        match self {
            Self::Number(value) if value.is_finite() => Ok(*value),
            Self::Integer(value) => Ok(f64::from(*value)),
            _ => Err(Error::Invalid("expected a finite scalar value".into()).into()),
        }
    }

    /// Treat only numeric zero or null as false for ordinary IL branches.
    fn truthy(&self) -> Result<bool> {
        match self {
            Self::Null => Ok(false),
            Self::Integer(value) => Ok(*value != 0),
            Self::Number(_) => {
                Err(Error::Invalid("floating value used as an IL branch condition".into()).into())
            }
            Self::This
            | Self::Property(_)
            | Self::Timeline(_)
            | Self::Collection(_)
            | Self::Frame(_)
            | Self::Easing(_)
            | Self::Random => Ok(true),
            Self::Forever => Err(Error::Invalid("repeat marker used as a condition".into()).into()),
        }
    }

    /// Require a completed timeline instead of silently accepting a void return.
    #[cfg(test)]
    fn timeline(&self) -> Result<Timeline> {
        match self {
            Self::Timeline(builder) => builder.borrow().finish(None),
            _ => Err(Error::Invalid("animation method did not produce a timeline".into()).into()),
        }
    }
}

/// Stateful but bounded evaluator for declarative animation factories.
struct Evaluator<'a> {
    /// The captured shader assembly, parsed without execution.
    assembly: &'a Assembly,
    /// Runtime effect type used to resolve inherited virtual methods.
    class: &'a str,
    /// Registered defaults recovered by the shared binding extractor.
    defaults: &'a BTreeMap<String, f64>,
    /// Static initialization values already recovered from selected slices.
    statics: BTreeMap<String, Value>,
    /// Static fields currently being resolved, preventing initialization cycles.
    resolving: BTreeSet<String>,
    /// Number of decoded instructions evaluated across recursive calls.
    steps: usize,
    /// Successfully recovered BeginAnimation assignments.
    tracks: Vec<AnimationTrack>,
    /// Whether startup requested pointer animation.
    mouse_loop: bool,
    /// Whether calls are currently being evaluated for the simulated hover event.
    hover_phase: bool,
    /// StopTimeAnimation calls actually reached under the supported hover state.
    hover_stops_time: bool,
    /// Timeline assignments performed inside those reached time-stop calls.
    hover_tracks: Vec<AnimationTrack>,
    /// Nesting of reached time-stop helpers while a hover event is being evaluated.
    time_stop_depth: usize,
}

impl<'a> Evaluator<'a> {
    /// Start one extraction with no managed runtime state.
    fn new(assembly: &'a Assembly, class: &'a str, defaults: &'a BTreeMap<String, f64>) -> Self {
        Self {
            assembly,
            class,
            defaults,
            statics: BTreeMap::new(),
            resolving: BTreeSet::new(),
            steps: 0,
            tracks: Vec::new(),
            mouse_loop: false,
            hover_phase: false,
            hover_stops_time: false,
            hover_tracks: Vec::new(),
            time_stop_depth: 0,
        }
    }

    /// Find a method through the declared inheritance chain.
    fn resolve(&self, owner: &str, name: &str, count: usize) -> Result<Method> {
        find_method(self.assembly, owner, name, count)
            .cloned()
            .ok_or_else(|| {
                unsupported(
                    &format!("{owner}::{name}"),
                    None,
                    "missing or ambiguous method",
                )
            })
    }

    /// Resolve a literal or one selected static-field initialization slice.
    fn static_value(&mut self, field: &FieldRef, depth: usize) -> Result<Value> {
        let key = format!("{}::{}", field.owner, field.name);
        if field.ty.ends_with("DependencyProperty") {
            return Ok(Value::Property(key));
        }
        if field.ty == "System.Random"
            || (field.name == "random" && field.owner == "ShaderEffects.Global")
        {
            return Ok(Value::Random);
        }
        if let Some(value) = self.statics.get(&key) {
            return Ok(value.clone());
        }
        let ty = self
            .assembly
            .types
            .get(&field.owner)
            .ok_or_else(|| unsupported(&key, None, "missing static-field type"))?;
        if let Some(constant) = ty
            .fields
            .get(&field.name)
            .and_then(|field| field.constant.as_ref())
        {
            return constant_value(constant);
        }
        if !self.resolving.insert(key.clone()) {
            return Err(unsupported(&key, None, "cyclic static initialization"));
        }
        let initializer = self.resolve(&field.owner, ".cctor", 0)?;
        let end = initializer.body.iter().position(|instruction| {
            instruction.opcode == OpCode::Stsfld && matches!(&instruction.operand, Operand::Field(reference) if reference.owner == field.owner && reference.name == field.name)
        }).ok_or_else(|| unsupported(&key, None, "missing static initialization"))?;
        let start = if field.ty.ends_with("Animation") || field.ty.ends_with("AnimationTimeline") {
            initializer.body[..end].iter().rposition(|instruction| {
                instruction.opcode == OpCode::Newobj && matches!(&instruction.operand, Operand::Method(reference) if is_timeline_type(&reference.owner))
            }).ok_or_else(|| unsupported(&key, None, "missing timeline constructor"))?
        } else {
            // Scalar field initializers occur after the previous complete static assignment.
            initializer.body[..end]
                .iter()
                .rposition(|instruction| instruction.opcode == OpCode::Stsfld)
                .map_or(0, |offset| offset + 1)
        };
        let mut slice = initializer.clone();
        slice.body = initializer.body[start..=end].to_vec();
        self.execute(&slice, Vec::new(), depth + 1)?;
        self.resolving.remove(&key);
        self.statics
            .get(&key)
            .cloned()
            .ok_or_else(|| unsupported(&key, None, "initializer did not assign its field"))
    }

    /// Evaluate supported instructions, preserving branches and object-reference identity.
    fn execute(&mut self, method: &Method, arguments: Vec<Value>, depth: usize) -> Result<Value> {
        let context = format!("{}::{}", method.owner, method.name);
        if depth > MAX_DEPTH || method.has_exception_handlers {
            return Err(unsupported(
                &context,
                None,
                "recursion or exception handling is unsupported",
            ));
        }
        if arguments.len() != method.parameter_count + usize::from(!method.is_static) {
            return Err(unsupported(
                &context,
                None,
                "argument count does not match method signature",
            ));
        }
        let mut stack = Vec::new();
        let mut locals = BTreeMap::new();
        let offsets: BTreeMap<_, _> = method
            .body
            .iter()
            .enumerate()
            .map(|(index, instruction)| (instruction.offset, index))
            .collect();
        let mut index = 0;
        while let Some(instruction) = method.body.get(index) {
            // Bound work across helper calls while preserving original IL offsets in errors.
            self.steps += 1;
            if self.steps > MAX_STEPS || stack.len() > MAX_SLOTS || locals.len() > MAX_SLOTS {
                return Err(unsupported(
                    &context,
                    Some(instruction.offset),
                    "animation extraction limit exceeded",
                ));
            }
            let fail = |detail: &str| unsupported(&context, Some(instruction.offset), detail);
            match instruction.opcode {
                OpCode::Nop => {}
                OpCode::LdcI4 => match instruction.operand {
                    Operand::Integer(value) => stack.push(Value::Integer(
                        i32::try_from(value)
                            .map_err(|_| fail("unsupported integer literal width"))?,
                    )),
                    _ => return Err(fail("invalid integral operand")),
                },
                OpCode::LdcR4 | OpCode::LdcR8 => match instruction.operand {
                    Operand::Float(value) => stack.push(Value::Number(value)),
                    _ => return Err(fail("invalid floating operand")),
                },
                OpCode::Ldnull => stack.push(Value::Null),
                OpCode::Ldarg => {
                    let Operand::Variable(slot) = instruction.operand else {
                        return Err(fail("invalid argument slot"));
                    };
                    stack.push(
                        arguments
                            .get(usize::from(slot))
                            .cloned()
                            .ok_or_else(|| fail("missing argument"))?,
                    );
                }
                OpCode::Ldloc | OpCode::Ldloca => {
                    let Operand::Variable(slot) = instruction.operand else {
                        return Err(fail("invalid local slot"));
                    };
                    stack.push(
                        locals
                            .get(&slot)
                            .cloned()
                            .ok_or_else(|| fail("uninitialized local"))?,
                    );
                }
                OpCode::Stloc => {
                    let Operand::Variable(slot) = instruction.operand else {
                        return Err(fail("invalid local slot"));
                    };
                    locals.insert(slot, pop(&mut stack, &context, instruction.offset)?);
                }
                OpCode::Dup => {
                    stack.push(stack.last().cloned().ok_or_else(|| fail("empty stack"))?)
                }
                OpCode::Pop => {
                    pop(&mut stack, &context, instruction.offset)?;
                }
                OpCode::Ldsfld => {
                    let Operand::Field(field) = &instruction.operand else {
                        return Err(fail("invalid static field"));
                    };
                    stack.push(self.static_value(field, depth + 1)?);
                }
                OpCode::Stsfld => {
                    let Operand::Field(field) = &instruction.operand else {
                        return Err(fail("invalid static field"));
                    };
                    self.statics.insert(
                        format!("{}::{}", field.owner, field.name),
                        pop(&mut stack, &context, instruction.offset)?,
                    );
                }
                OpCode::Add
                | OpCode::Sub
                | OpCode::Mul
                | OpCode::Div
                | OpCode::Rem
                | OpCode::Ceq
                | OpCode::Cgt
                | OpCode::Clt
                | OpCode::CgtUn
                | OpCode::CltUn => {
                    // Preserve integer or floating semantics and attach the source offset to failures.
                    let right = pop(&mut stack, &context, instruction.offset)?;
                    let left = pop(&mut stack, &context, instruction.offset)?;
                    stack.push(
                        binary(instruction.opcode, left, right)
                            .map_err(|error| fail(&error.to_string()))?,
                    );
                }
                OpCode::Neg => {
                    let value = match pop(&mut stack, &context, instruction.offset)? {
                        Value::Integer(value) => Value::Integer(
                            value
                                .checked_neg()
                                .ok_or_else(|| fail("integer negation overflow"))?,
                        ),
                        Value::Number(value) => Value::Number(-value),
                        _ => return Err(fail("nonnumeric negation")),
                    };
                    stack.push(value);
                }
                OpCode::Box | OpCode::Castclass => {}
                OpCode::UnboxAny => {
                    // Recover the property's numeric category from its declared unboxed type.
                    let Operand::Type(ty) = &instruction.operand else {
                        return Err(fail("missing unboxed type"));
                    };
                    let value = pop(&mut stack, &context, instruction.offset)?;
                    let integer = matches!(
                        ty.as_str(),
                        "int32" | "bool" | "System.Int32" | "System.Boolean"
                    ) || self
                        .assembly
                        .types
                        .get(ty)
                        .is_some_and(|ty| ty.base.as_deref() == Some("System.Enum"));
                    if integer {
                        // Reject fractional or out-of-range values instead of coercing an enum or integer.
                        let value = value.number()?;
                        if value < f64::from(i32::MIN)
                            || value > f64::from(i32::MAX)
                            || value.fract() != 0.0
                        {
                            return Err(fail("invalid unboxed integer"));
                        }
                        stack.push(Value::Integer(value as i32));
                    } else if matches!(ty.as_str(), "float64" | "System.Double") {
                        stack.push(Value::Number(value.number()?));
                    } else {
                        return Err(fail("unsupported unboxed type"));
                    }
                }
                OpCode::ConvR8 | OpCode::ConvR4 | OpCode::ConvI4 => {
                    // Keep CLI truncation and single-precision rounding explicit at conversions.
                    let value = pop(&mut stack, &context, instruction.offset)?.number()?;
                    let value = match instruction.opcode {
                        OpCode::ConvR8 => Value::Number(value),
                        OpCode::ConvR4 => Value::Number(f64::from(value as f32)),
                        OpCode::ConvI4
                            if value.trunc() >= f64::from(i32::MIN)
                                && value.trunc() <= f64::from(i32::MAX) =>
                        {
                            Value::Integer(value as i32)
                        }
                        _ => return Err(fail("numeric conversion exceeds supported range")),
                    };
                    stack.push(value);
                }
                OpCode::Br
                | OpCode::Brtrue
                | OpCode::Brfalse
                | OpCode::Beq
                | OpCode::BneUn
                | OpCode::Bge
                | OpCode::Bgt
                | OpCode::Ble
                | OpCode::Blt
                | OpCode::BgeUn
                | OpCode::BgtUn
                | OpCode::BleUn
                | OpCode::BltUn => {
                    let take = match instruction.opcode {
                        OpCode::Br => true,
                        OpCode::Brtrue => {
                            pop(&mut stack, &context, instruction.offset)?.truthy()?
                        }
                        OpCode::Brfalse => {
                            !pop(&mut stack, &context, instruction.offset)?.truthy()?
                        }
                        opcode => {
                            // Choose the edge using this instruction's signed or unsigned comparison.
                            let right = pop(&mut stack, &context, instruction.offset)?;
                            let left = pop(&mut stack, &context, instruction.offset)?;
                            let unsigned = matches!(
                                opcode,
                                OpCode::BgeUn
                                    | OpCode::BgtUn
                                    | OpCode::BleUn
                                    | OpCode::BltUn
                                    | OpCode::BneUn
                            );
                            let order = numeric_order(&left, &right, unsigned)?;
                            match opcode {
                                OpCode::Beq => order.is_eq(),
                                OpCode::BneUn => !order.is_eq(),
                                OpCode::Bge | OpCode::BgeUn => !order.is_lt(),
                                OpCode::Bgt | OpCode::BgtUn => order.is_gt(),
                                OpCode::Ble | OpCode::BleUn => !order.is_gt(),
                                _ => order.is_lt(),
                            }
                        }
                    };
                    if take {
                        index = target(instruction, &offsets, &context)?;
                        continue;
                    }
                }
                OpCode::Switch => {
                    // Negative and out-of-range CLI selectors follow the switch's fall-through edge.
                    let Value::Integer(selector) = pop(&mut stack, &context, instruction.offset)?
                    else {
                        return Err(fail("switch requires an integer selector"));
                    };
                    let Operand::Switch(targets) = &instruction.operand else {
                        return Err(fail("invalid switch"));
                    };
                    if let Some(target) = usize::try_from(selector)
                        .ok()
                        .and_then(|selector| targets.get(selector))
                    {
                        index = *offsets
                            .get(target)
                            .ok_or_else(|| fail("invalid switch target"))?;
                        continue;
                    }
                }
                OpCode::Newobj | OpCode::Call | OpCode::Callvirt => {
                    // Consume receiver and arguments according to the decoded method signature.
                    let Operand::Method(reference) = &instruction.operand else {
                        return Err(fail("invalid method operand"));
                    };
                    if stack.len() < reference.parameter_count {
                        return Err(fail("missing call arguments"));
                    }
                    let arguments = stack.split_off(stack.len() - reference.parameter_count);
                    let receiver = if instruction.opcode != OpCode::Newobj && reference.has_this {
                        Some(pop(&mut stack, &context, instruction.offset)?)
                    } else {
                        None
                    };
                    let value = if instruction.opcode == OpCode::Newobj {
                        self.construct(reference, arguments)?
                    } else {
                        self.call(
                            reference,
                            receiver,
                            arguments,
                            depth + 1,
                            instruction.opcode == OpCode::Callvirt,
                        )?
                    };
                    if instruction.opcode == OpCode::Newobj || reference.returns_value {
                        stack.push(value);
                    }
                }
                OpCode::Ret => {
                    return if method.returns_value {
                        pop(&mut stack, &context, instruction.offset)
                    } else {
                        Ok(Value::Null)
                    };
                }
                _ => return Err(fail(&format!("instruction {}", instruction.opcode.name()))),
            }
            index += 1;
        }
        Ok(Value::Null)
    }

    /// Build only recognized WPF data objects, without running their constructors.
    fn construct(&self, reference: &MethodRef, arguments: Vec<Value>) -> Result<Value> {
        let owner = reference.owner.as_str();
        if is_timeline_type(owner) {
            // Construct custom range data only after proving its interpolation contract.
            if owner == "ShaderEffects.IntervalDoubleAnimation" {
                super::interval::validate(self.assembly)?;
            }
            let (curve, duration, explicit_from, explicit_to) = match (owner, arguments.as_slice())
            {
                ("System.Windows.Media.Animation.DoubleAnimationUsingKeyFrames", []) => {
                    (Curve::Keyframes(Vec::new()), None, false, false)
                }
                (_, []) => (
                    Curve::Range { from: 0.0, to: 0.0 },
                    None,
                    owner == "ShaderEffects.IntervalDoubleAnimation",
                    owner == "ShaderEffects.IntervalDoubleAnimation",
                ),
                ("System.Windows.Media.Animation.DoubleAnimation", [to, duration]) => (
                    Curve::Range {
                        from: 0.0,
                        to: to.number()?,
                    },
                    Some(duration.number()?),
                    false,
                    true,
                ),
                ("System.Windows.Media.Animation.DoubleAnimation", [from, to, duration]) => (
                    Curve::Range {
                        from: from.number()?,
                        to: to.number()?,
                    },
                    Some(duration.number()?),
                    true,
                    true,
                ),
                _ => {
                    return Err(unsupported(
                        owner,
                        None,
                        "unsupported animation constructor signature",
                    ));
                }
            };
            return Ok(Value::Timeline(Rc::new(RefCell::new(Builder {
                curve,
                explicit_from,
                explicit_to,
                duration,
                repeat: false,
                auto_reverse: false,
                acceleration: 0.0,
                deceleration: 0.0,
            }))));
        }
        if matches!(
            owner,
            "System.Windows.Media.Animation.LinearDoubleKeyFrame"
                | "System.Windows.Media.Animation.DiscreteDoubleKeyFrame"
                | "System.Windows.Media.Animation.EasingDoubleKeyFrame"
        ) && arguments.len() == 2
        {
            let interpolation = if owner.ends_with("DiscreteDoubleKeyFrame") {
                Interpolation::Discrete
            } else {
                Interpolation::Linear
            };
            return Ok(Value::Frame(Rc::new(RefCell::new(Frame {
                frame: Keyframe {
                    value: arguments[0].number()?,
                    seconds: arguments[1].number()?,
                    interpolation,
                },
                easing: None,
            }))));
        }
        if owner == "System.Windows.Media.Animation.SineEase" && arguments.is_empty() {
            // WPF's default is EaseOut; explicit EasingMode setters override it.
            return Ok(Value::Easing(Rc::new(Cell::new(1))));
        }
        if owner == "System.Windows.Duration" && arguments.len() == 1 {
            return Ok(arguments[0].clone());
        }
        Err(unsupported(
            &format!("{}::.ctor", reference.owner),
            None,
            "unrecognized animation object",
        ))
    }

    /// Evaluate recognized framework operations or another captured helper method.
    fn call(
        &mut self,
        reference: &MethodRef,
        receiver: Option<Value>,
        arguments: Vec<Value>,
        depth: usize,
        virtual_call: bool,
    ) -> Result<Value> {
        let owner = reference.owner.as_str();
        let name = reference.name.as_str();
        if name == "get_ShowAnimation" {
            return Ok(Value::Integer(1));
        }
        if name == "get_AttachedElement" {
            return Ok(Value::Null);
        }
        if name == "StartMouseAnimation" {
            self.mouse_loop = true;
        }
        if name == "ResetMousePos" {
            validate_pointer_reset(self.assembly, self.class)?;
            return Ok(Value::Null);
        }
        if owner == "System.Random" && name == "NextDouble" {
            return Ok(Value::Number(0.0));
        }
        if (owner == "System.TimeSpan" && name == "FromSeconds")
            || (owner == "System.Windows.KeyTime" && name == "FromTimeSpan")
            || (owner == "System.Windows.Media.Animation.KeyTime" && name == "FromTimeSpan")
            || (owner == "System.Windows.Duration" && name == "op_Implicit")
        {
            return arguments
                .first()
                .cloned()
                .ok_or_else(|| unsupported(name, None, "missing timing argument"));
        }
        if owner == "System.TimeSpan" && name == "FromMilliseconds" {
            return Ok(Value::Number(arguments[0].number()? / 1000.0));
        }
        if owner == "System.Windows.Media.Animation.RepeatBehavior" && name == "get_Forever" {
            return Ok(Value::Forever);
        }
        if let Some(Value::Timeline(builder)) = &receiver {
            // Mutate the shared builder so duplicated IL references retain object identity.
            let mut builder = builder.borrow_mut();
            match name {
                "get_KeyFrames" => {
                    drop(builder);
                    return Ok(Value::Collection(match receiver {
                        Some(Value::Timeline(builder)) => builder,
                        _ => unreachable!(),
                    }));
                }
                "set_Duration" => builder.duration = Some(arguments[0].number()?),
                "set_AutoReverse" => builder.auto_reverse = arguments[0].truthy()?,
                "set_AccelerationRatio" => builder.acceleration = arguments[0].number()?,
                "set_DecelerationRatio" => builder.deceleration = arguments[0].number()?,
                "set_RepeatBehavior" if matches!(arguments.first(), Some(Value::Forever)) => {
                    builder.repeat = true
                }
                "set_From" => {
                    match &mut builder.curve {
                        Curve::Range { from, .. } => *from = arguments[0].number()?,
                        _ => return Err(unsupported(name, None, "range setter on keyframes")),
                    }
                    builder.explicit_from = true;
                }
                "set_To" => {
                    match &mut builder.curve {
                        Curve::Range { to, .. } => *to = arguments[0].number()?,
                        _ => return Err(unsupported(name, None, "range setter on keyframes")),
                    }
                    builder.explicit_to = true;
                }
                _ => {
                    return Err(unsupported(
                        &format!("{owner}::{name}"),
                        None,
                        "unsupported timeline operation",
                    ));
                }
            }
            return Ok(Value::Null);
        }
        if let Some(Value::Collection(builder)) = &receiver
            && name == "Add"
        {
            // Freeze the keyframe's final easing configuration when it enters the collection.
            let Some(Value::Frame(frame)) = arguments.first() else {
                return Err(unsupported(name, None, "expected scalar keyframe"));
            };
            let frame = frame.borrow();
            let mut value = frame.frame.clone();
            if let Some(mode) = &frame.easing {
                value.interpolation = match mode.get() {
                    0 => Interpolation::SineIn,
                    1 => Interpolation::SineOut,
                    2 => Interpolation::SineInOut,
                    _ => return Err(unsupported(name, None, "unknown easing mode")),
                };
            }
            let mut builder = builder.borrow_mut();
            let Curve::Keyframes(frames) = &mut builder.curve else {
                return Err(unsupported(
                    name,
                    None,
                    "collection on non-keyframe animation",
                ));
            };
            if frames.len() >= MAX_KEYFRAMES {
                return Err(unsupported(name, None, "keyframe limit exceeded"));
            }
            frames.push(value);
            return Ok(Value::Integer((frames.len() - 1) as i32));
        }
        if let Some(Value::Frame(frame)) = &receiver
            && name == "set_EasingFunction"
        {
            // Retain the easing object because its mode can be assigned after construction.
            let Some(Value::Easing(easing)) = arguments.first() else {
                return Err(unsupported(name, None, "unknown easing function"));
            };
            frame.borrow_mut().easing = Some(easing.clone());
            return Ok(Value::Null);
        }
        if let Some(Value::Easing(easing)) = &receiver
            && name == "set_EasingMode"
        {
            easing.set(arguments[0].number()? as i32);
            return Ok(Value::Null);
        }
        if name == "BeginAnimation" && owner == "System.Windows.Media.Animation.Animatable" {
            // Resolve property resets and replacement semantics before collecting a track.
            let Some(Value::Property(property)) = arguments.first() else {
                return Err(unsupported(
                    name,
                    None,
                    "unknown animated dependency property",
                ));
            };
            let pointer =
                property.ends_with("::MouseXProperty") || property.ends_with("::MouseYProperty");
            let timeline = match arguments.get(1) {
                Some(Value::Null) if pointer => None,
                Some(Value::Null) => {
                    let value =
                        self.defaults.get(property).copied().ok_or_else(|| {
                            unsupported(property, None, "missing scalar reset value")
                        })?;
                    Some(Timeline {
                        curve: Curve::Range {
                            from: value,
                            to: value,
                        },
                        duration: 0.0,
                        repeat: false,
                        auto_reverse: false,
                        acceleration: 0.0,
                        deceleration: 0.0,
                    })
                }
                Some(Value::Timeline(builder)) => Some(
                    builder
                        .borrow()
                        .finish(self.defaults.get(property).copied())?,
                ),
                _ => return Err(unsupported(name, None, "unsupported animated value")),
            };
            if pointer {
                self.mouse_loop = true;
            } else if let Some(timeline) = timeline {
                // Replace earlier property animations and retain hover time-stop assignments.
                let track = AnimationTrack {
                    property: property.clone(),
                    timeline,
                };
                self.tracks.retain(|track| track.property != *property);
                self.tracks.push(track.clone());
                if self.time_stop_depth > 0 {
                    self.hover_tracks
                        .retain(|track| track.property != *property);
                    self.hover_tracks.push(track);
                }
            }
            return Ok(Value::Null);
        }
        if name == "GetValue" && owner == "System.Windows.DependencyObject" {
            let Some(Value::Property(property)) = arguments.first() else {
                return Err(unsupported(name, None, "unknown dependency property"));
            };
            return self
                .defaults
                .get(property)
                .copied()
                .map(Value::Number)
                .ok_or_else(|| unsupported(property, None, "missing scalar default"));
        }
        let target_owner =
            if virtual_call && matches!(receiver, Some(Value::This)) && name != ".ctor" {
                self.class
            } else {
                owner
            };
        let method = self.resolve(target_owner, name, reference.parameter_count)?;
        let mut values = arguments;
        if reference.has_this {
            values.insert(
                0,
                receiver.ok_or_else(|| unsupported(name, None, "missing receiver"))?,
            );
        }
        let stopping_time = self.hover_phase && name == "StopTimeAnimation";
        if stopping_time {
            self.hover_stops_time = true;
            self.time_stop_depth += 1;
        }
        let result = self.execute(&method, values, depth);
        if stopping_time {
            self.time_stop_depth -= 1;
        }
        result
    }
}

/// Resolve one signature without guessing between ambiguous overloads.
fn find_method<'a>(
    assembly: &'a Assembly,
    owner: &str,
    name: &str,
    count: usize,
) -> Option<&'a Method> {
    let mut owner = owner;
    let mut visited = BTreeSet::new();
    while visited.insert(owner) {
        // Follow inheritance without choosing between ambiguous same-arity overloads.
        let ty = assembly.types.get(owner)?;
        let mut methods = ty
            .methods
            .iter()
            .filter(|method| method.name == name && method.parameter_count == count);
        if let Some(method) = methods.next() {
            return if methods.next().is_none() {
                Some(method)
            } else {
                None
            };
        }
        owner = ty.base.as_deref()?;
    }
    None
}

/// Recognize a pointer-only reset before replacing its value with standard preview policy.
fn validate_pointer_reset(assembly: &Assembly, class: &str) -> Result<()> {
    let reset = find_method(assembly, class, "ResetMousePos", 0)
        .ok_or_else(|| unsupported(class, None, "missing pointer reset"))?;
    let body: Vec<_> = reset
        .body
        .iter()
        .filter(|instruction| instruction.opcode != OpCode::Nop)
        .collect();
    let operations = [
        OpCode::Ldarg,
        OpCode::Ldsfld,
        OpCode::Ldarg,
        OpCode::Callvirt,
        OpCode::Box,
        OpCode::Call,
        OpCode::Ret,
    ];
    if body
        .iter()
        .map(|instruction| instruction.opcode)
        .collect::<Vec<_>>()
        != operations
        || !matches!(&body[1].operand, Operand::Field(field) if field.name == "MousePosProperty")
        || !matches!(&body[3].operand, Operand::Method(method) if method.name == "get_DefaultMousePosition")
        || !matches!(&body[5].operand, Operand::Method(method) if method.owner == "System.Windows.DependencyObject" && method.name == "SetValue")
    {
        return Err(unsupported(
            class,
            None,
            "pointer reset has additional unsupported behavior",
        ));
    }
    let getter = find_method(assembly, class, "get_DefaultMousePosition", 0)
        .ok_or_else(|| unsupported(class, None, "missing default pointer value"))?;
    if getter
        .body
        .iter()
        .filter(|instruction| instruction.opcode != OpCode::Nop)
        .map(|instruction| instruction.opcode)
        .collect::<Vec<_>>()
        != [OpCode::Ldarg, OpCode::Ldfld, OpCode::Ret]
    {
        return Err(unsupported(
            class,
            None,
            "default pointer getter has unsupported side effects",
        ));
    }
    Ok(())
}

/// Restrict constructor recognition to the supported standard and client timeline shapes.
fn is_timeline_type(owner: &str) -> bool {
    matches!(
        owner,
        "System.Windows.Media.Animation.DoubleAnimationUsingKeyFrames"
            | "System.Windows.Media.Animation.DoubleAnimation"
            | "ShaderEffects.IntervalDoubleAnimation"
    )
}

/// Convert metadata literals to the subset meaningful for animation construction.
fn constant_value(value: &Constant) -> Result<Value> {
    match value {
        Constant::Integer(value) => {
            Ok(Value::Integer(i32::try_from(*value).map_err(|_| {
                Error::Invalid("unsupported integer literal width".into())
            })?))
        }
        Constant::Float(value) => Ok(Value::Number(*value)),
        Constant::Null => Ok(Value::Null),
        Constant::String(_) => {
            Err(Error::Invalid("string used as numeric animation data".into()).into())
        }
    }
}

/// Pop a stack value with its originating method and byte-offset diagnostic.
fn pop(stack: &mut Vec<Value>, context: &str, offset: u32) -> Result<Value> {
    stack
        .pop()
        .ok_or_else(|| unsupported(context, Some(offset), "evaluation stack underflow"))
}

/// Require a branch target to match a decoded instruction boundary.
fn target(
    instruction: &Instruction,
    offsets: &BTreeMap<u32, usize>,
    context: &str,
) -> Result<usize> {
    let Operand::Branch(offset) = instruction.operand else {
        return Err(unsupported(
            context,
            Some(instruction.offset),
            "invalid branch operand",
        ));
    };
    offsets
        .get(&offset)
        .copied()
        .ok_or_else(|| unsupported(context, Some(instruction.offset), "invalid branch target"))
}

/// Evaluate the supported typed integer and floating-point arithmetic operations.
fn binary(opcode: OpCode, left: Value, right: Value) -> Result<Value> {
    if opcode == OpCode::Ceq {
        let value = match (&left, &right) {
            (Value::Null, Value::Null) | (Value::This, Value::This) => true,
            (Value::Null, _) | (_, Value::Null) => false,
            _ => numeric_order(&left, &right, false)?.is_eq(),
        };
        return Ok(Value::Integer(i32::from(value)));
    }
    if opcode == OpCode::CgtUn && matches!(right, Value::Null) {
        return Ok(Value::Integer(i32::from(left.truthy()?)));
    }
    if matches!(
        opcode,
        OpCode::Cgt | OpCode::Clt | OpCode::CgtUn | OpCode::CltUn
    ) {
        let order = numeric_order(
            &left,
            &right,
            matches!(opcode, OpCode::CgtUn | OpCode::CltUn),
        )?;
        return Ok(Value::Integer(i32::from(
            if matches!(opcode, OpCode::Cgt | OpCode::CgtUn) {
                order.is_gt()
            } else {
                order.is_lt()
            },
        )));
    }
    match (left, right) {
        (Value::Integer(left), Value::Integer(right)) => {
            let value = match opcode {
                OpCode::Add => left.checked_add(right),
                OpCode::Sub => left.checked_sub(right),
                OpCode::Mul => left.checked_mul(right),
                OpCode::Div => left.checked_div(right),
                OpCode::Rem => left.checked_rem(right),
                _ => None,
            }
            .ok_or_else(|| Error::Invalid("unsupported integer overflow or division".into()))?;
            Ok(Value::Integer(value))
        }
        (Value::Number(left), Value::Number(right)) => {
            // Reject nonfinite arithmetic before it can affect timelines or control flow.
            let value = match opcode {
                OpCode::Add => left + right,
                OpCode::Sub => left - right,
                OpCode::Mul => left * right,
                OpCode::Div => left / right,
                OpCode::Rem => left % right,
                _ => return Err(Error::Invalid("unknown floating operation".into()).into()),
            };
            if !value.is_finite() {
                return Err(Error::Invalid("nonfinite arithmetic result".into()).into());
            }
            Ok(Value::Number(value))
        }
        _ => Err(Error::Invalid("unsupported mixed integer/floating arithmetic".into()).into()),
    }
}

/// Compare supported stack categories without lossy integer-to-float conversion.
fn numeric_order(left: &Value, right: &Value, unsigned: bool) -> Result<std::cmp::Ordering> {
    match (left, right) {
        (Value::Integer(left), Value::Integer(right)) => Ok(if unsigned {
            (*left as u32).cmp(&(*right as u32))
        } else {
            left.cmp(right)
        }),
        (Value::Number(left), Value::Number(right)) => left
            .partial_cmp(right)
            .ok_or_else(|| Error::Invalid("nonfinite floating comparison".into()).into()),
        _ => Err(Error::Invalid("unsupported mixed numeric comparison".into()).into()),
    }
}

/// Preserve the precise unsupported method and operation without swallowing errors.
fn unsupported(context: &str, offset: Option<u32>, detail: &str) -> Box<dyn std::error::Error> {
    Error::Unsupported {
        context: context.into(),
        offset,
        detail: detail.into(),
    }
    .into()
}

#[cfg(test)]
mod tests;
