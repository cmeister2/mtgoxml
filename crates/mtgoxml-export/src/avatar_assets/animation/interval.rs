//! Recognize the client's cached linear range primitive by its decoded data flow.
//!
//! `IntervalDoubleAnimation` is client code, so its class name alone cannot prove
//! linear interpolation. Both return paths must compute `From + (To - From) *
//! progress`, using current or cached clock progress. A new formula is unsupported.

use std::collections::{BTreeMap, BTreeSet};

use super::Error;
use super::il::{Assembly, Method, OpCode, Operand};
use crate::Result;

/// The specific custom primitive whose supported semantics are checked here.
const INTERVAL: &str = "ShaderEffects.IntervalDoubleAnimation";

/// Symbolic values proving interpolation and clock-refresh branch roles.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expression {
    /// The animation's declared start value.
    From,
    /// The animation's declared end value.
    To,
    /// Normalized WPF clock progress on an update.
    CurrentProgress,
    /// Previously cached progress, identified by its actual field.
    CachedProgress(String),
    /// Current wall-clock time used solely for the cache refresh decision.
    Now,
    /// Previous wall-clock update time, identified by its actual field.
    LastUpdate(String),
    /// The uninitialized wall-clock sentinel.
    MinTime,
    /// The declared clock update interval.
    UpdateInterval,
    /// Elapsed time since a particular cached update.
    Elapsed(String),
    /// Whether the clock-refresh interval elapsed.
    RefreshDue(String),
    /// Whether a particular cached clock has never been updated.
    NeverUpdated(String),
    /// Nonarithmetic receiver values.
    Other,
    /// Scalar addition retained without numerical approximation.
    Add(Box<Self>, Box<Self>),
    /// Scalar subtraction retained without numerical approximation.
    Subtract(Box<Self>, Box<Self>),
    /// Scalar multiplication retained without numerical approximation.
    Multiply(Box<Self>, Box<Self>),
}

/// Require both interpolation expressions and their supported clock/cache control flow.
///
/// # Errors
/// Returns an unsupported-pattern error for changed interpolation, unrecognized
/// clock bookkeeping, or an update interval larger than preview sampling.
pub(super) fn validate(assembly: &Assembly) -> Result<()> {
    let ty = assembly
        .types
        .get(INTERVAL)
        .ok_or_else(|| fail("missing custom range type"))?;
    let method = ty
        .methods
        .iter()
        .find(|method| method.name == "GetCurrentValue" && method.parameter_count == 3)
        .ok_or_else(|| fail("missing GetCurrentValue method"))?;
    if method.has_exception_handlers || method.body.len() > 512 {
        return Err(fail("unsupported custom range control flow"));
    }
    validate_accessors(&ty.methods)?;
    let mut stack = Vec::new();
    let mut locals = BTreeMap::new();
    let mut cached_progress = BTreeSet::new();
    let mut updated_clocks = BTreeSet::new();
    let mut returns = Vec::new();
    let mut branches = Vec::new();
    for (index, instruction) in method.body.iter().enumerate() {
        match instruction.opcode {
            OpCode::Nop | OpCode::Box | OpCode::UnboxAny | OpCode::Castclass | OpCode::ConvR8 => {}
            OpCode::Ldarg => stack.push(Expression::Other),
            OpCode::Ldloc | OpCode::Ldloca => {
                let Operand::Variable(slot) = instruction.operand else {
                    return Err(fail("invalid symbolic local"));
                };
                stack.push(
                    locals
                        .get(&slot)
                        .cloned()
                        .ok_or_else(|| fail("uninitialized symbolic local"))?,
                );
            }
            OpCode::Stloc => {
                let Operand::Variable(slot) = instruction.operand else {
                    return Err(fail("invalid symbolic local"));
                };
                locals.insert(slot, pop(&mut stack)?);
            }
            OpCode::Ldsfld => match &instruction.operand {
                Operand::Field(field)
                    if field.owner == "System.DateTime" && field.name == "MinValue" =>
                {
                    stack.push(Expression::MinTime)
                }
                _ => return Err(fail("unknown custom range static state")),
            },
            OpCode::Ldfld => {
                // Keep cached progress distinct from wall-clock state in the interpolation proof.
                pop(&mut stack)?;
                let Operand::Field(field) = &instruction.operand else {
                    return Err(fail("invalid symbolic field"));
                };
                stack.push(if cached_progress.contains(&field.name) {
                    Expression::CachedProgress(field.name.clone())
                } else if field.owner == INTERVAL && field.ty == "System.DateTime" {
                    Expression::LastUpdate(field.name.clone())
                } else {
                    Expression::Other
                });
            }
            OpCode::Stfld => {
                // Associate stored values with the exact fields used by the two return paths.
                let value = pop(&mut stack)?;
                pop(&mut stack)?;
                let Operand::Field(field) = &instruction.operand else {
                    return Err(fail("invalid symbolic field"));
                };
                match value {
                    Expression::CurrentProgress => {
                        cached_progress.insert(field.name.clone());
                    }
                    Expression::Now if field.owner == INTERVAL && field.ty == "System.DateTime" => {
                        updated_clocks.insert(field.name.clone());
                    }
                    _ => return Err(fail("unrecognized custom clock state update")),
                }
            }
            OpCode::Call | OpCode::Callvirt => {
                // Preserve call argument order while recognizing only supported clock operations.
                let Operand::Method(reference) = &instruction.operand else {
                    return Err(fail("invalid symbolic call"));
                };
                if stack.len() < reference.parameter_count {
                    return Err(fail("symbolic argument underflow"));
                }
                let arguments = stack.split_off(stack.len() - reference.parameter_count);
                let receiver = if reference.has_this {
                    Some(pop(&mut stack)?)
                } else {
                    None
                };
                let value = match (
                    reference.owner.as_str(),
                    reference.name.as_str(),
                    arguments.as_slice(),
                ) {
                    (INTERVAL, "get_From", []) => Expression::From,
                    (INTERVAL, "get_To", []) => Expression::To,
                    ("System.Windows.Media.Animation.Clock", "get_CurrentProgress", []) => {
                        Expression::CurrentProgress
                    }
                    (owner, "get_Value", []) if owner.starts_with("System.Nullable`1") => {
                        receiver.ok_or_else(|| fail("missing clock progress receiver"))?
                    }
                    (INTERVAL, "get_UpdateInterval", []) => Expression::UpdateInterval,
                    ("System.DateTime", "get_Now", []) => Expression::Now,
                    (
                        "System.DateTime",
                        "op_Subtraction",
                        [Expression::Now, Expression::LastUpdate(field)],
                    ) => Expression::Elapsed(field.clone()),
                    (
                        "System.TimeSpan",
                        "op_GreaterThanOrEqual",
                        [Expression::Elapsed(field), Expression::UpdateInterval],
                    ) => Expression::RefreshDue(field.clone()),
                    (
                        "System.DateTime",
                        "op_Equality",
                        [Expression::LastUpdate(field), Expression::MinTime],
                    ) => Expression::NeverUpdated(field.clone()),
                    _ => return Err(fail("unknown operation in custom interpolation")),
                };
                if reference.returns_value {
                    stack.push(value);
                }
            }
            OpCode::Sub | OpCode::Mul | OpCode::Add => {
                // Build symbolic expressions so a changed interpolation formula cannot pass.
                let right = Box::new(pop(&mut stack)?);
                let left = Box::new(pop(&mut stack)?);
                stack.push(match instruction.opcode {
                    OpCode::Sub => Expression::Subtract(left, right),
                    OpCode::Mul => Expression::Multiply(left, right),
                    _ => Expression::Add(left, right),
                });
            }
            OpCode::Brtrue | OpCode::Brfalse => {
                // Retain branch polarity and targets to detect swapped refresh and cache paths.
                let Operand::Branch(target) = instruction.operand else {
                    return Err(fail("invalid clock refresh branch"));
                };
                branches.push((instruction.opcode, pop(&mut stack)?, target, index));
                if !stack.is_empty() {
                    return Err(fail("unexpected branch stack state"));
                }
            }
            OpCode::Ret => {
                returns.push((pop(&mut stack)?, index));
                if !stack.is_empty() {
                    return Err(fail("unexpected return stack state"));
                }
            }
            _ => {
                return Err(fail(&format!(
                    "unknown custom interpolation instruction {}",
                    instruction.opcode.name()
                )));
            }
        }
    }
    if returns.len() != 2
        || branches.len() != 2
        || cached_progress.len() != 1
        || updated_clocks.len() != 1
    {
        return Err(fail(
            "custom range is not the supported current/cached progress pattern",
        ));
    }
    let clock = updated_clocks
        .iter()
        .next()
        .ok_or_else(|| fail("missing updated clock"))?;
    let cache = cached_progress
        .iter()
        .next()
        .ok_or_else(|| fail("missing progress cache"))?;
    let update = method
        .body
        .get(branches[1].3 + 1)
        .ok_or_else(|| fail("missing clock update block"))?
        .offset;
    let cached = method
        .body
        .get(returns[0].1 + 1)
        .ok_or_else(|| fail("missing cached interpolation block"))?
        .offset;
    if branches[0].0 != OpCode::Brtrue
        || branches[0].1 != Expression::RefreshDue(clock.clone())
        || branches[0].2 != update
        || branches[1].0 != OpCode::Brfalse
        || branches[1].1 != Expression::NeverUpdated(clock.clone())
        || branches[1].2 != cached
        || returns[0].0 != linear(Expression::CurrentProgress)
        || returns[1].0 != linear(Expression::CachedProgress(cache.clone()))
    {
        return Err(fail(
            "unsupported custom interpolation or clock-refresh branch topology",
        ));
    }
    validate_refresh(ty.methods.as_slice(), clock)
}

/// Construct the exact supported interpolation expression for one progress source.
fn linear(progress: Expression) -> Expression {
    Expression::Add(
        Box::new(Expression::From),
        Box::new(Expression::Multiply(
            Box::new(Expression::Subtract(
                Box::new(Expression::To),
                Box::new(Expression::From),
            )),
            Box::new(progress),
        )),
    )
}

/// Require only the supported cache initialization and a refresh interval below one frame.
fn validate_refresh(methods: &[Method], clock: &str) -> Result<()> {
    let getter = methods
        .iter()
        .find(|method| method.name == "get_UpdateInterval" && method.parameter_count == 0)
        .ok_or_else(|| fail("missing update interval getter"))?;
    let getter: Vec<_> = getter
        .body
        .iter()
        .filter(|instruction| instruction.opcode != OpCode::Nop)
        .collect();
    if getter
        .iter()
        .map(|instruction| instruction.opcode)
        .collect::<Vec<_>>()
        != [OpCode::Ldarg, OpCode::Ldfld, OpCode::Ret]
    {
        return Err(fail(
            "update interval getter performs unsupported calculations",
        ));
    }
    let Operand::Field(backing) = &getter[1].operand else {
        return Err(fail("missing update interval field"));
    };
    let constructor = methods
        .iter()
        .find(|method| method.name == ".ctor" && method.parameter_count == 0)
        .ok_or_else(|| fail("missing custom range constructor"))?;
    if constructor.has_exception_handlers {
        return Err(fail("custom constructor has exception semantics"));
    }
    let body: Vec<_> = constructor
        .body
        .iter()
        .filter(|instruction| instruction.opcode != OpCode::Nop)
        .collect();
    let operations = [
        OpCode::Ldarg,
        OpCode::LdcR8,
        OpCode::Call,
        OpCode::Stfld,
        OpCode::Ldarg,
        OpCode::Ldsfld,
        OpCode::Stfld,
        OpCode::Ldarg,
        OpCode::Call,
        OpCode::Ret,
    ];
    if body
        .iter()
        .map(|instruction| instruction.opcode)
        .collect::<Vec<_>>()
        != operations
    {
        return Err(fail(
            "custom constructor has unsupported clock or cache initialization",
        ));
    }
    if ![0, 4, 7]
        .into_iter()
        .all(|index| body[index].operand == Operand::Variable(0))
        || !matches!(&body[3].operand, Operand::Field(field) if field == backing)
        || !matches!(&body[5].operand, Operand::Field(field) if field.owner == "System.DateTime" && field.name == "MinValue")
        || !matches!(&body[6].operand, Operand::Field(field) if field.owner == INTERVAL && field.name == clock && field.ty == "System.DateTime")
        || !matches!(&body[8].operand, Operand::Method(method) if method.owner == "System.Windows.Media.Animation.AnimationTimeline" && method.name == ".ctor" && method.parameter_count == 0)
    {
        return Err(fail(
            "custom constructor no longer initializes the supported empty cache",
        ));
    }
    let Operand::Float(value) = body[1].operand else {
        return Err(fail("nonconstant update interval"));
    };
    let Operand::Method(reference) = &body[2].operand else {
        return Err(fail("unknown update interval conversion"));
    };
    let seconds = match (reference.owner.as_str(), reference.name.as_str()) {
        ("System.TimeSpan", "FromMilliseconds") => value / 1000.0,
        ("System.TimeSpan", "FromSeconds") => value,
        _ => return Err(fail("unknown update interval conversion")),
    };
    if !seconds.is_finite() || !(0.0..=0.1).contains(&seconds) {
        return Err(fail(
            "custom update interval exceeds the supported preview sample interval",
        ));
    }
    Ok(())
}

/// Require From and To to remain ordinary scalar dependency-property accessors.
fn validate_accessors(methods: &[Method]) -> Result<()> {
    for property in ["From", "To"] {
        for (prefix, operations, callback) in [
            (
                "get_",
                &[
                    OpCode::Ldarg,
                    OpCode::Ldsfld,
                    OpCode::Call,
                    OpCode::UnboxAny,
                    OpCode::Ret,
                ][..],
                "GetValue",
            ),
            (
                "set_",
                &[
                    OpCode::Ldarg,
                    OpCode::Ldsfld,
                    OpCode::Ldarg,
                    OpCode::Box,
                    OpCode::Call,
                    OpCode::Ret,
                ][..],
                "SetValue",
            ),
        ] {
            // Prove that each accessor uses its own registered property without extra calculations.
            let name = format!("{prefix}{property}");
            let method = methods
                .iter()
                .find(|method| method.name == name)
                .ok_or_else(|| fail("missing range property accessor"))?;
            let body: Vec<_> = method
                .body
                .iter()
                .filter(|instruction| instruction.opcode != OpCode::Nop)
                .collect();
            if body
                .iter()
                .map(|instruction| instruction.opcode)
                .collect::<Vec<_>>()
                != operations
            {
                return Err(fail(
                    "custom range property accessor performs unsupported calculations",
                ));
            }
            if !matches!(&body[1].operand, Operand::Field(field) if field.owner == INTERVAL && field.name == format!("{property}Property"))
                || !body.iter().any(|instruction| matches!(&instruction.operand, Operand::Method(reference) if reference.owner == "System.Windows.DependencyObject" && reference.name == callback))
            {
                return Err(fail("custom range property does not use its registered value"));
            }
        }
    }
    let initializer = methods
        .iter()
        .find(|method| method.name == ".cctor")
        .ok_or_else(|| fail("missing custom range static initialization"))?;
    for instruction in &initializer.body {
        if let Operand::Method(reference) = &instruction.operand {
            if reference.owner == "System.Windows.DependencyProperty"
                && reference.name == "Register"
                && reference.parameter_count != 3
            {
                return Err(fail(
                    "custom range defaults require unsupported property metadata",
                ));
            }
        }
    }
    Ok(())
}

/// Read a symbolic stack value without accepting malformed IL.
fn pop(stack: &mut Vec<Expression>) -> Result<Expression> {
    stack.pop().ok_or_else(|| fail("symbolic stack underflow"))
}

/// Preserve a precise unsupported-pattern diagnostic for the custom primitive.
fn fail(detail: &str) -> Box<dyn std::error::Error> {
    Error::Unsupported {
        context: format!("{INTERVAL}::GetCurrentValue"),
        offset: None,
        detail: detail.into(),
    }
    .into()
}
