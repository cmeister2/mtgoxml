//! Interpret captured declarations with modeled WPF properties and bounded state.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use super::super::super::Result;
use super::super::il::{
    Assembly, Constant as Literal, FieldRef, Method, MethodRef, OpCode, Operand,
};
use super::{Bindings, Constant, Source};

/// Supported framework calls used by declaration constructors and callbacks.
mod framework;
/// Typed instruction evaluation, including branches and stack semantics.
mod interpret;

/// Values represent only the metadata and image operations supported by extraction.
#[derive(Debug, Clone)]
enum Value {
    /// A null reference or an uninitialized nonnumeric reference.
    Null,
    /// An integral literal or enum discriminator.
    Integer(i64),
    /// A floating-point literal or computed scalar.
    Number(f64),
    /// A boxed value retains its nonnull reference identity, including enum zero.
    Boxed(Box<Value>, String),
    /// A string literal.
    Text(String),
    /// A managed type token.
    Type(String),
    /// A method pointer used by a property callback.
    Method(MethodRef),
    /// A modeled managed object in the evaluator heap.
    Object(usize),
    /// A registered dependency property.
    Property(usize),
    /// Metadata supplying a property's default value and callback.
    Metadata(Box<Value>, Option<Box<Value>>),
    /// A shader constant's float-register callback.
    Register(usize),
    /// A two- or three-component managed vector.
    Vector(Vec<f64>),
    /// A symbolic mouse component used to prove an identity property route.
    Pointer,
    /// A vector carrying an unchanged symbolic mouse component.
    PointerVector(Vec<Value>),
    /// An original resource URI.
    Uri(String),
    /// A resolved shader input.
    Image(Source),
    /// A mutable local/argument address, used by value-type constructors.
    Address(Rc<RefCell<Value>>),
    /// A modeled array in the heap.
    Array(usize),
    /// The known nonnull avatar supplied to the control.
    Avatar,
    /// Its shader-data projection.
    ShaderData,
    /// A dependency-property change event.
    Change(Box<Value>),
    /// The assembly owning a managed type token.
    Assembly(String),
    /// An irrelevant value that must never determine an extracted binding.
    Unknown,
}

impl Value {
    /// Read a numeric enum or integer operand without coercing unknown values.
    fn integer(&self) -> Result<i64> {
        match self {
            Self::Integer(value) => Ok(*value),
            Self::Address(value) => value.borrow().integer(),
            Self::Boxed(value, _) => value.integer(),
            _ => Err(format!("expected an IL integer, got {self:?}").into()),
        }
    }

    /// Read a concrete scalar used by metadata constructors.
    fn number(&self) -> Result<f64> {
        match self {
            Self::Integer(value) => Ok(*value as f64),
            Self::Number(value) => Ok(*value),
            Self::Address(value) => value.borrow().number(),
            Self::Boxed(value, _) => value.number(),
            _ => Err(format!("expected an IL number, got {self:?}").into()),
        }
    }

    /// Read a known string operand.
    fn text(&self) -> Result<&str> {
        match self {
            Self::Text(value) | Self::Uri(value) => Ok(value),
            _ => Err(format!("expected an IL string, got {self:?}").into()),
        }
    }

    /// Locate a modeled object's heap entry.
    fn object(&self) -> Result<usize> {
        match self {
            Self::Object(index) => Ok(*index),
            Self::Address(value) => value.borrow().object(),
            _ => Err(format!("expected an IL object, got {self:?}").into()),
        }
    }

    /// Resolve CLI branch truth only for concrete values.
    fn truth(&self) -> Result<bool> {
        match self {
            Self::Null => Ok(false),
            Self::Integer(value) => Ok(*value != 0),
            Self::Number(value) => Ok(*value != 0.0),
            Self::Unknown | Self::Pointer | Self::PointerVector(_) => {
                Err("unsupported unknown value controls a binding branch".into())
            }
            _ => Ok(true),
        }
    }

    /// Convert a dependency-property default to its shader register representation.
    fn vector(&self) -> Result<Vec<f64>> {
        match self {
            Self::Integer(value) => Ok(vec![*value as f64]),
            Self::Number(value) => Ok(vec![*value]),
            Self::Vector(values) => Ok(values.clone()),
            Self::Boxed(value, _) => value.vector(),
            _ => Err(format!("unsupported shader constant value {self:?}").into()),
        }
    }
}

/// State of one managed effect or small WPF carrier object.
#[derive(Debug, Clone)]
struct Object {
    /// Runtime type used for inheritance and virtual calls.
    ty: String,
    /// Ordinary managed instance fields.
    fields: BTreeMap<String, Value>,
    /// Explicit dependency-property values, overriding metadata defaults.
    values: BTreeMap<usize, Value>,
    /// Properties explicitly connected to the pixel shader.
    active: BTreeMap<usize, usize>,
    /// URI installed through ShaderEffect.PixelShader.
    shader: Option<String>,
}

/// One dependency property's registration facts.
#[derive(Debug, Clone)]
struct Property {
    /// Original static field, assigned after registration.
    field: String,
    /// Registered property name.
    name: String,
    /// Original property's value type.
    ty: String,
    /// Metadata default, before instance and avatar changes.
    default: Value,
    /// Callback connecting the property to a shader register or managed method.
    callback: Option<Value>,
    /// Sampler number for brush properties.
    sampler: Option<u32>,
}

/// Bounded IL state; it models declarations instead of hosting a CLR.
pub(super) struct Machine<'a> {
    /// Assemblies already verified by the exporter.
    assemblies: &'a [Assembly],
    /// Original catalog premium discriminator.
    effect: u32,
    /// Converted EffectTypes value used by AvatarControl.
    effect_type: i64,
    /// Both possible frame-helper answers must agree for avatars.
    frame_support: bool,
    /// Registered properties created by static constructors.
    properties: Vec<Property>,
    /// Modeled object and array heaps.
    objects: Vec<Object>,
    /// Array elements used by ordinary string-construction IL.
    arrays: Vec<Vec<Value>>,
    /// Aggregate array allocation remains independent of individual array bounds.
    array_elements: usize,
    /// Static field values populated by `.cctor` interpretation.
    fields: BTreeMap<String, Value>,
    /// Types whose static constructor is in progress or complete.
    initialized: BTreeSet<String>,
    /// Total instruction budget across recursive calls.
    fuel: usize,
}

impl<'a> Machine<'a> {
    /// Resolve the avatar factory and collect its constructed effect metadata.
    pub(super) fn extract(self) -> Result<Bindings> {
        let mut machine = self;
        let definition = machine.object("WotC.MtGO.Client.Model.Core.CardDefinition");
        let getter = machine.method(
            "WotC.MtGO.Client.Model.Core.CardDefinition",
            "get_PremiumEffect",
            0,
        )?;
        machine.frame_support = false;
        let kind = machine
            .run(&getter, vec![definition.clone()], 0)?
            .integer()?;
        machine.frame_support = true;
        if machine.run(&getter, vec![definition], 0)?.integer()? != kind {
            return Err("avatar effect mapping depends on unsupported frame state".into());
        }
        machine.effect_type = kind;
        let control_type = "WotC.MtGO.Client.Model.Core.Controls.AvatarControl";
        let control = machine.object(control_type);
        let control_id = control.object()?;
        machine.objects[control_id].fields.insert(
            format!("{control_type}::m_lastEffectType"),
            Value::Integer(-1),
        );
        let apply = machine.method(control_type, "ApplyShaderEffect", 0)?;
        machine.run(&apply, vec![control], 0)?;
        let selected = machine.objects[control_id]
            .fields
            .get(&format!("{control_type}::m_effect"))
            .ok_or("avatar control did not assign an effect")?
            .object()?;
        machine.finish(selected)
    }

    /// Reserve bounded state for one effect extraction.
    pub(super) fn new(assemblies: &'a [Assembly], effect: u32) -> Self {
        Self {
            assemblies,
            effect,
            effect_type: 0,
            frame_support: false,
            properties: Vec::new(),
            objects: Vec::new(),
            arrays: Vec::new(),
            array_elements: 0,
            fields: BTreeMap::new(),
            initialized: BTreeSet::new(),
            fuel: 200_000,
        }
    }

    /// Find one captured type rather than loading a runtime dependency.
    fn ty(&self, name: &str) -> Option<&super::super::il::Type> {
        self.assemblies
            .iter()
            .find_map(|assembly| assembly.types.get(name))
    }

    /// Find an exact method or inherited implementation by its declared arity.
    fn method(&self, owner: &str, name: &str, count: usize) -> Result<Method> {
        let mut current = owner.to_owned();
        let mut visited = BTreeSet::new();
        while visited.len() < 64 && visited.insert(current.clone()) {
            // Search inherited declarations and reject ambiguous overloads.
            let ty = self
                .ty(&current)
                .ok_or_else(|| format!("missing captured IL type {current}"))?;
            let mut matches = ty
                .methods
                .iter()
                .filter(|method| method.name == name && method.parameter_count == count);
            if let Some(method) = matches.next() {
                if matches.next().is_some() {
                    return Err(format!(
                        "ambiguous captured IL overload {current}::{name}/{count}"
                    )
                    .into());
                }
                return Ok(method.clone());
            }
            match &ty.base {
                Some(base) => current = base.clone(),
                None => break,
            }
        }
        Err(format!("missing captured IL method {owner}::{name}/{count}").into())
    }

    /// Follow managed inheritance while recognizing the final external base.
    fn inherits(&self, ty: &str, base: &str) -> bool {
        let mut current = ty;
        let mut visited = BTreeSet::new();
        while visited.len() < 64 && visited.insert(current) {
            if current == base {
                return true;
            }
            match self.ty(current).and_then(|ty| ty.base.as_deref()) {
                Some(parent) => current = parent,
                None => break,
            }
        }
        false
    }

    /// Allocate a simple managed object without invoking external constructors.
    fn object(&mut self, ty: &str) -> Value {
        let index = self.objects.len();
        self.objects.push(Object {
            ty: ty.into(),
            fields: BTreeMap::new(),
            values: BTreeMap::new(),
            active: BTreeMap::new(),
            shader: None,
        });
        Value::Object(index)
    }

    /// Build the canonical identity used by field and dependency-property references.
    fn field(field: &FieldRef) -> String {
        format!("{}::{}", field.owner, field.name)
    }

    /// Initialize each captured static constructor at most once.
    fn initialize(&mut self, owner: &str, depth: usize) -> Result<()> {
        if !self.initialized.insert(owner.into()) {
            return Ok(());
        }
        if let Some(ty) = self.ty(owner).cloned() {
            // Install CLR zero and literal values before running this static initializer.
            if let Some(base) = &ty.base {
                self.initialize(base, depth + 1)?;
            }
            for field in ty.fields.values() {
                let value = match &field.constant {
                    Some(Literal::Integer(value)) => Value::Integer(*value),
                    Some(Literal::Float(value)) => Value::Number(*value),
                    Some(Literal::String(value)) => Value::Text(value.clone()),
                    _ => default_value(&field.ty),
                };
                self.fields
                    .entry(format!("{owner}::{}", field.name))
                    .or_insert(value);
            }
            if let Some(method) = ty.methods.iter().find(|method| method.name == ".cctor") {
                self.run(method, Vec::new(), depth + 1)?;
            }
        }
        Ok(())
    }

    /// Get an explicit dependency value or its registration default.
    fn value(&self, object: usize, property: usize) -> Value {
        self.objects[object]
            .values
            .get(&property)
            .cloned()
            .unwrap_or_else(|| self.properties[property].default.clone())
    }

    /// Assign a property and interpret its declared property-change callback.
    fn set_value(
        &mut self,
        object: usize,
        property: usize,
        value: Value,
        depth: usize,
    ) -> Result<()> {
        self.objects[object].values.insert(property, value.clone());
        if let Some(Value::Method(callback)) = self.properties[property].callback.clone() {
            let method = self.method(&callback.owner, &callback.name, callback.parameter_count)?;
            self.run(
                &method,
                vec![Value::Object(object), Value::Change(Box::new(value))],
                depth + 1,
            )?;
        }
        Ok(())
    }

    /// Resolve a modeled property operand.
    fn property(value: &Value) -> Result<usize> {
        match value {
            Value::Property(index) => Ok(*index),
            _ => Err(format!("expected a dependency property, got {value:?}").into()),
        }
    }

    /// Assemble typed bindings only from fully resolved shader assignments.
    fn finish(&mut self, object: usize) -> Result<Bindings> {
        let mut constants = BTreeMap::new();
        let mut samplers = BTreeMap::new();
        for (property, _) in self.objects[object].active.clone() {
            let registration = self.properties[property].clone();
            if let Some(sampler) = registration.sampler {
                let source = if sampler == 0 && registration.name == "Input" {
                    Source::Input
                } else {
                    match self.value(object, property) {
                        Value::Image(source) => source,
                        value => {
                            return Err(format!(
                                "sampler {} has unsupported source {value:?}",
                                registration.field
                            )
                            .into());
                        }
                    }
                };
                if samplers.insert(sampler, source).is_some() {
                    return Err("multiple active properties assign one sampler".into());
                }
            }
        }
        for property in 0..self.properties.len() {
            // Include properties whose shader constants may be activated by later animation.
            let registration = self.properties[property].clone();
            let owner = registration.field.split("::").next().unwrap_or_default();
            if !self.inherits(&self.objects[object].ty, owner) {
                continue;
            }
            if let Some(Value::Register(register)) = registration.callback {
                // Validate the declared vector shape before retaining the initial register value.
                let vector = self.value(object, property).vector()?;
                if register >= 224
                    || vector.is_empty()
                    || vector.len() > 4
                    || vector
                        .iter()
                        .any(|value| !value.is_finite() || !(*value as f32).is_finite())
                {
                    return Err("shader constant exceeds supported register bounds".into());
                }
                let expected = match registration.ty.as_str() {
                    "System.Windows.Point" => 2,
                    "System.Windows.Media.Media3D.Point3D" => 3,
                    "System.Double" | "System.Single" | "float64" | "float32" => 1,
                    other => return Err(format!("unsupported shader constant type {other}").into()),
                };
                if vector.len() != expected {
                    return Err(format!(
                        "shader constant {} has inconsistent dimensions",
                        registration.field
                    )
                    .into());
                }
                let mut default = [1.0; 4];
                for (slot, value) in default.iter_mut().zip(&vector) {
                    *slot = *value as f32;
                }
                constants.insert(
                    registration.field,
                    Constant {
                        register,
                        component: 0,
                        components: vector.len(),
                        default,
                        priority: self.objects[object]
                            .active
                            .get(&property)
                            .copied()
                            .unwrap_or(0),
                    },
                );
            }
        }
        // An identity marker follows the original callback into its vector component.
        // Arithmetic or branches on the marker are unsupported rather than approximated.
        let original = self.objects.clone();
        for property in 0..self.properties.len() {
            // Follow an identity marker through each mouse callback to its register component.
            let registration = self.properties[property].clone();
            if !matches!(registration.name.as_str(), "MouseX" | "MouseY") {
                continue;
            }
            if !self.inherits(
                &self.objects[object].ty,
                registration.field.split("::").next().unwrap_or_default(),
            ) {
                continue;
            }
            self.set_value(object, property, Value::Pointer, 0)?;
            let mut routes = Vec::new();
            for (active, _) in self.objects[object].active.clone() {
                if let Some(Value::Register(register)) = self.properties[active].callback {
                    let components = pointer_components(&self.value(object, active));
                    for component in components {
                        routes.push((active, register, component));
                    }
                }
            }
            if routes.len() != 1 {
                return Err(
                    format!("unsupported mouse-property route {}", registration.field).into(),
                );
            }
            let (active, register, component) = routes[0];
            let before = original[object]
                .values
                .get(&active)
                .cloned()
                .unwrap_or_else(|| self.properties[active].default.clone())
                .vector()?;
            let mut default = [1.0; 4];
            for (slot, value) in default.iter_mut().zip(before) {
                *slot = value as f32;
            }
            constants.insert(
                registration.field,
                Constant {
                    register,
                    component,
                    components: 1,
                    default,
                    priority: 0,
                },
            );
            self.objects = original.clone();
        }
        let scalar_defaults = self
            .properties
            .iter()
            .enumerate()
            .filter_map(|(index, property)| {
                // Retain scalar settings only for properties applicable to this effect class.
                let owner = property.field.split("::").next().unwrap_or_default();
                if !self.inherits(&self.objects[object].ty, owner) {
                    return None;
                }
                match self.value(object, index) {
                    Value::Integer(value) => Some((property.field.clone(), value as f64)),
                    Value::Number(value) => Some((property.field.clone(), value)),
                    Value::Boxed(value, _) => value
                        .number()
                        .ok()
                        .map(|value| (property.field.clone(), value)),
                    _ => None,
                }
            })
            .collect();
        Ok(Bindings {
            class: self.objects[object].ty.clone(),
            shader_resource: self.objects[object]
                .shader
                .clone()
                .ok_or("effect did not assign a pixel shader")?,
            samplers,
            constants,
            scalar_defaults,
        })
    }
}

/// Resolve an IL branch target without accepting offsets into another instruction.
fn target_index(method: &Method, offset: u32) -> Result<usize> {
    method
        .body
        .iter()
        .position(|instruction| instruction.offset == offset)
        .ok_or_else(|| format!("invalid IL branch target {offset}").into())
}

/// Compare concrete CLI values used by enum, null, and string branches.
fn compare(left: &Value, right: &Value) -> Result<i8> {
    let ordering = match (left, right) {
        (Value::Unknown, _) | (_, Value::Unknown) => {
            return Err("unknown value controls an IL comparison".into());
        }
        (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
        (Value::Null, _) => std::cmp::Ordering::Less,
        (_, Value::Null) => std::cmp::Ordering::Greater,
        (Value::Object(left), Value::Object(right)) => left.cmp(right),
        (Value::Integer(left), Value::Integer(right)) => left.cmp(right),
        (Value::Text(left), Value::Text(right)) => left.cmp(right),
        _ => left
            .number()?
            .partial_cmp(&right.number()?)
            .ok_or("NaN controls an IL binding branch")?,
    };
    Ok(match ordering {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    })
}

/// Evaluate concrete arithmetic and comparisons used by declaration constructors.
fn binary(opcode: OpCode, left: Value, right: Value) -> Result<Value> {
    if matches!(opcode, OpCode::CgtUn | OpCode::CltUn) {
        unsigned_operands(&left, &right)?;
    }
    if let (Value::Integer(first), Value::Integer(last)) = (&left, &right) {
        let result = match opcode {
            OpCode::Add => Some(first.checked_add(*last)),
            OpCode::Sub => Some(first.checked_sub(*last)),
            OpCode::Mul => Some(first.checked_mul(*last)),
            OpCode::Div => Some(first.checked_div(*last)),
            OpCode::Rem => Some(first.checked_rem(*last)),
            _ => None,
        };
        if let Some(result) = result {
            // Preserve integer arithmetic and reject overflow with ambiguous stack width.
            let result = result.ok_or("unsupported overflow or division in binding IL")?;
            if i32::try_from(*first).is_ok()
                && i32::try_from(*last).is_ok()
                && i32::try_from(result).is_err()
            {
                return Err(
                    "binding IL arithmetic requires unsupported integer overflow semantics".into(),
                );
            }
            return Ok(Value::Integer(result));
        }
    }
    match opcode {
        OpCode::Ceq => Ok(Value::Integer(i64::from(compare(&left, &right)? == 0))),
        OpCode::Cgt | OpCode::CgtUn => Ok(Value::Integer(i64::from(compare(&left, &right)? > 0))),
        OpCode::Clt | OpCode::CltUn => Ok(Value::Integer(i64::from(compare(&left, &right)? < 0))),
        OpCode::And => Ok(Value::Integer(left.integer()? & right.integer()?)),
        OpCode::Or => Ok(Value::Integer(left.integer()? | right.integer()?)),
        OpCode::Xor => Ok(Value::Integer(left.integer()? ^ right.integer()?)),
        OpCode::Add => Ok(Value::Number(left.number()? + right.number()?)),
        OpCode::Sub => Ok(Value::Number(left.number()? - right.number()?)),
        OpCode::Mul => Ok(Value::Number(left.number()? * right.number()?)),
        OpCode::Div => Ok(Value::Number(left.number()? / right.number()?)),
        OpCode::Rem => Ok(Value::Number(left.number()? % right.number()?)),
        _ => Err(format!("unsupported IL binary operator {opcode:?}").into()),
    }
}

/// Reject signed-negative values when their original unsigned stack width is unavailable.
fn unsigned_operands(left: &Value, right: &Value) -> Result<()> {
    if [left, right]
        .iter()
        .any(|value| matches!(value, Value::Integer(value) if *value < 0))
    {
        return Err("unsupported unsigned comparison of negative binding IL integers".into());
    }
    Ok(())
}

/// Implement supported CLI numeric conversions without silently retaining source precision.
fn convert(opcode: OpCode, value: Value) -> Result<Value> {
    match opcode {
        OpCode::ConvR8 => Ok(Value::Number(value.number()?)),
        OpCode::ConvR4 => {
            // Round to actual single precision before retaining the scalar.
            let value = value.number()? as f32;
            if !value.is_finite() {
                return Err("binding IL conversion exceeds finite float32".into());
            }
            Ok(Value::Number(f64::from(value)))
        }
        OpCode::ConvI4 => match value {
            Value::Integer(value) => Ok(Value::Integer(i64::from(value as i32))),
            value => {
                // Truncate finite floats only within the destination's signed bounds.
                let value = value.number()?.trunc();
                if !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX)
                {
                    return Err("unsupported float-to-int32 conversion in binding IL".into());
                }
                Ok(Value::Integer(value as i64))
            }
        },
        OpCode::ConvU4 => {
            // Reject unsigned values whose original stack width would be ambiguous.
            let value = value.number()?.trunc();
            if !value.is_finite() || value < 0.0 || value > f64::from(i32::MAX) {
                return Err(
                    "binding IL conversion requires unsupported unsigned stack width".into(),
                );
            }
            Ok(Value::Integer(value as i64))
        }
        OpCode::ConvI8 | OpCode::ConvU8 => match value {
            Value::Integer(value) if opcode == OpCode::ConvI8 || value >= 0 => {
                Ok(Value::Integer(value))
            }
            value => {
                // Check representability before converting a floating-point value.
                let value = value.number()?.trunc();
                if !value.is_finite()
                    || value
                        < if opcode == OpCode::ConvU8 {
                            0.0
                        } else {
                            i64::MIN as f64
                        }
                    || value >= i64::MAX as f64
                {
                    return Err("unsupported float-to-int64 conversion in binding IL".into());
                }
                Ok(Value::Integer(value as i64))
            }
        },
        _ => Err(format!("unsupported binding IL conversion {opcode:?}").into()),
    }
}

/// CLR zero values needed before static and instance initializers run.
fn default_value(ty: &str) -> Value {
    match ty {
        "System.Windows.Point" => Value::Vector(vec![0.0; 2]),
        "System.Windows.Media.Media3D.Point3D" => Value::Vector(vec![0.0; 3]),
        "System.Double" | "System.Single" | "float64" | "float32" => Value::Number(0.0),
        "System.Boolean" | "System.Int32" | "System.Int64" | "int32" | "int64" | "bool" => {
            Value::Integer(0)
        }
        _ => Value::Null,
    }
}

/// Match a WPF pack URI to the case-normalized embedded resource exported by ILSpy.
fn resource_path(uri: &str) -> Result<String> {
    let prefix = "pack://application:,,,/";
    let (assembly, resource) = uri
        .strip_prefix(prefix)
        .and_then(|uri| uri.split_once(";component/"))
        .ok_or("unsupported client resource URI")?;
    if assembly.is_empty()
        || !assembly
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        || matches!(assembly, "." | "..")
        || resource
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || resource.contains(['\\', ':', '\0'])
    {
        return Err("invalid client resource path".into());
    }
    Ok(format!(
        "decompiled/{assembly}/{}",
        resource.to_ascii_lowercase()
    ))
}

/// Build a vector while preserving a symbolic pointer's exact component identity.
fn vector_value(values: &[Value]) -> Result<Value> {
    if values.iter().any(|value| matches!(value, Value::Pointer)) {
        for value in values {
            if !matches!(value, Value::Pointer) {
                value.number()?;
            }
        }
        Ok(Value::PointerVector(values.to_vec()))
    } else {
        Ok(Value::Vector(
            values.iter().map(Value::number).collect::<Result<_>>()?,
        ))
    }
}

/// Follow CLR boxing while locating unchanged symbolic vector components.
fn pointer_components(value: &Value) -> Vec<usize> {
    match value {
        Value::Pointer => vec![0],
        Value::PointerVector(values) => values
            .iter()
            .enumerate()
            .filter_map(|(index, value)| matches!(value, Value::Pointer).then_some(index))
            .collect(),
        Value::Boxed(value, _) => pointer_components(value),
        _ => Vec::new(),
    }
}
