//! Model the framework declarations that connect client assets to shader registers.

use super::*;

impl Machine<'_> {
    /// Model known framework calls and recurse only through captured declaration code.
    pub(super) fn call(
        &mut self,
        method: &MethodRef,
        args: Vec<Value>,
        constructor: bool,
        virtual_call: bool,
        depth: usize,
    ) -> Result<Value> {
        let owner = method.owner.as_str();
        let name = method.name.as_str();
        if constructor {
            // Construct modeled framework carriers or interpret captured constructors.
            match owner {
                "System.Windows.PropertyChangedCallback" => {
                    return match args.as_slice() {
                        [Value::Null, Value::Method(callback)]
                            if !callback.has_this
                                && callback.parameter_count == 2
                                && !callback.returns_value =>
                        {
                            Ok(Value::Method(callback.clone()))
                        }
                        _ => Err("unsupported bound or nonstatic property-change callback".into()),
                    };
                }
                "System.Windows.UIPropertyMetadata" | "System.Windows.PropertyMetadata" => {
                    return Ok(Value::Metadata(
                        Box::new(args.first().cloned().unwrap_or(Value::Null)),
                        args.get(1).cloned().map(Box::new),
                    ));
                }
                "System.Windows.Point" | "System.Windows.Media.Media3D.Point3D" => {
                    return vector_value(&args);
                }
                "System.Uri" => {
                    return Ok(Value::Uri(
                        args.first().ok_or("missing resource URI")?.text()?.into(),
                    ));
                }
                "System.Windows.Media.Imaging.BitmapImage" => {
                    return Ok(Value::Image(Source::Shared(resource_path(
                        args.first().ok_or("missing image URI")?.text()?,
                    )?)));
                }
                "System.Windows.Media.ImageBrush" => {
                    return args
                        .into_iter()
                        .next()
                        .ok_or_else(|| "missing image brush source".into());
                }
                _ => {}
            }
            if owner.starts_with("System.Windows.Media.Animation.")
                || self.inherits(owner, "System.Windows.Media.Animation.AnimationTimeline")
                || matches!(owner, "System.Windows.Duration" | "System.Random")
            {
                return Ok(Value::Unknown);
            }
            if self.ty(owner).is_none()
                && !matches!(
                    owner,
                    "System.Windows.Data.Binding" | "System.Windows.Media.Effects.PixelShader"
                )
            {
                return Err(format!(
                    "unsupported binding constructor {owner}/{}",
                    method.parameter_count
                )
                .into());
            }
            self.initialize(owner, depth + 1)?;
            let object = self.object(owner);
            if self.ty(owner).is_some() {
                // Evaluate the captured constructor so its field and property writes are retained.
                let mut constructor_args = vec![object.clone()];
                constructor_args.extend(args);
                let implementation = self.method(owner, name, method.parameter_count)?;
                self.run(&implementation, constructor_args, depth + 1)?;
            } else if owner == "System.Windows.Data.Binding" {
                self.objects[object.object()?]
                    .fields
                    .insert("path".into(), args.first().cloned().unwrap_or(Value::Null));
            }
            return Ok(object);
        }
        if name == ".ctor" {
            // Write value-type receivers directly and interpret captured base constructors.
            if matches!(
                owner,
                "System.Windows.Point" | "System.Windows.Media.Media3D.Point3D"
            ) {
                let value = vector_value(&args[1..])?;
                if let Some(Value::Address(destination)) = args.first() {
                    *destination.borrow_mut() = value;
                    return Ok(Value::Null);
                }
            }
            if self.ty(owner).is_some() {
                let implementation = self.method(owner, name, method.parameter_count)?;
                return self.run(&implementation, args, depth + 1);
            }
            if matches!(
                owner,
                "System.Object"
                    | "System.Windows.DependencyObject"
                    | "System.Windows.Freezable"
                    | "System.Windows.Media.Animation.Animatable"
                    | "System.Windows.Media.Effects.Effect"
                    | "System.Windows.Media.Effects.ShaderEffect"
            ) {
                return Ok(Value::Null);
            }
            return Err(format!("unsupported binding base constructor {owner}").into());
        }
        if owner == "System.Type" && name == "GetTypeFromHandle" {
            return args
                .into_iter()
                .next()
                .ok_or_else(|| "missing type handle".into());
        }
        if owner == "System.Type" && name == "get_Assembly" {
            // Resolve the token's captured assembly without loading a runtime dependency.
            let Value::Type(ty) = &args[0] else {
                return Err("unsupported reflected type".into());
            };
            let assembly = self
                .assemblies
                .iter()
                .find(|assembly| assembly.types.contains_key(ty))
                .ok_or("missing reflected assembly")?;
            return Ok(Value::Assembly(assembly.name.clone()));
        }
        if name == "ToString" {
            if let Some(Value::Assembly(name)) = args.first() {
                return Ok(Value::Text(name.clone()));
            }
        }
        if owner == "System.String" {
            match name {
                "Concat" => {
                    // Resolve every concrete fragment before bounding the combined string allocation.
                    let values = if let [Value::Array(index)] = args.as_slice() {
                        self.arrays[*index].clone()
                    } else {
                        args
                    };
                    let parts = values
                        .iter()
                        .map(|value| match value {
                            Value::Null => Ok(""),
                            _ => value.text(),
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let length = parts
                        .iter()
                        .try_fold(0_usize, |length, part| length.checked_add(part.len()))
                        .ok_or("binding IL string length overflow")?;
                    if length > 4 * 1024 * 1024 {
                        return Err("binding IL string exceeds its bound".into());
                    }
                    return Ok(Value::Text(parts.join("")));
                }
                "Split" => {
                    // Honor the declared separator array and preserve ordinary empty split entries.
                    if args.len() != 2 {
                        return Err("unsupported string split overload in binding IL".into());
                    }
                    let Value::Array(separators) = args[1] else {
                        return Err(
                            "binding IL string split requires an explicit character array".into(),
                        );
                    };
                    let separators = self.arrays[separators]
                        .iter()
                        .map(|value| {
                            char::from_u32(u32::try_from(value.integer()?)?)
                                .ok_or_else(|| "unsupported UTF-16 separator in binding IL".into())
                        })
                        .collect::<Result<Vec<_>>>()?;
                    if separators.is_empty() {
                        return Err("binding IL string split requires explicit separators".into());
                    }
                    let source = args[0].text()?;
                    let values: Vec<_> = source
                        .split(|character| separators.contains(&character))
                        .take(4097)
                        .map(|text| Value::Text(text.to_owned()))
                        .collect();
                    self.array_elements = self
                        .array_elements
                        .checked_add(values.len())
                        .ok_or("binding IL array allocation overflow")?;
                    if values.len() > 4096 || self.array_elements > 65_536 {
                        return Err("binding IL string split exceeds its bound".into());
                    }
                    let index = self.arrays.len();
                    self.arrays.push(values);
                    return Ok(Value::Array(index));
                }
                "op_Equality" | "op_Inequality" => {
                    let equal = compare(&args[0], &args[1])? == 0;
                    return Ok(Value::Integer(i64::from(equal == (name == "op_Equality"))));
                }
                _ => {}
            }
        }
        if owner == "System.Windows.Media.Effects.ShaderEffect" {
            match name {
                "RegisterPixelShaderSamplerProperty" => {
                    // Retain declared sampler numbers and reject unsupported sampling modes.
                    if args.len() != 3 && !(args.len() == 4 && args[3].integer()? == 0) {
                        return Err(
                            "unsupported shader sampling mode in captured binding IL".into()
                        );
                    }
                    let property_name = args[0].text()?.to_owned();
                    let Value::Type(ty) = &args[1] else {
                        return Err("missing sampler owner type".into());
                    };
                    let index = self.properties.len();
                    self.properties.push(Property {
                        field: format!("{ty}::{property_name}Property"),
                        name: property_name,
                        ty: "System.Windows.Media.Brush".into(),
                        default: Value::Null,
                        callback: None,
                        sampler: Some(u32::try_from(args[2].integer()?)?),
                    });
                    return Ok(Value::Property(index));
                }
                "PixelShaderConstantCallback" => {
                    return Ok(Value::Register(usize::try_from(args[0].integer()?)?));
                }
                "UpdateShaderValue" => {
                    // Retain installation order so later properties take register priority.
                    let object = args[0].object()?;
                    let property = Self::property(&args[1])?;
                    let priority = self.objects[object]
                        .active
                        .values()
                        .copied()
                        .max()
                        .unwrap_or(0)
                        + 1;
                    self.objects[object].active.insert(property, priority);
                    return Ok(Value::Null);
                }
                "set_PixelShader" => {
                    // Read the original URI installed on this modeled effect instance.
                    let object = args[0].object()?;
                    let shader = args[1].object()?;
                    let uri = self.objects[shader]
                        .fields
                        .get("UriSource")
                        .ok_or("pixel shader is missing its URI")?
                        .text()?;
                    self.objects[object].shader = Some(resource_path(uri)?);
                    return Ok(Value::Null);
                }
                _ => {}
            }
        }
        if owner == "System.Windows.DependencyProperty" && name == "Register" {
            // Record the property's original type, default and callback.
            let property_name = args[0].text()?.to_owned();
            let Value::Type(ty) = &args[1] else {
                return Err("missing registered property type".into());
            };
            let Value::Type(declaring) = &args[2] else {
                return Err("missing registered property owner".into());
            };
            let (default, callback) = match args.get(3) {
                Some(Value::Metadata(value, callback)) => (
                    (**value).clone(),
                    callback.as_ref().map(|value| (**value).clone()),
                ),
                _ => (default_value(ty), None),
            };
            let index = self.properties.len();
            self.properties.push(Property {
                field: format!("{declaring}::{property_name}Property"),
                name: property_name,
                ty: ty.clone(),
                default,
                callback,
                sampler: None,
            });
            return Ok(Value::Property(index));
        }
        if owner == "System.Windows.DependencyObject" {
            match name {
                "GetValue" => return Ok(self.value(args[0].object()?, Self::property(&args[1])?)),
                "SetValue" => {
                    self.set_value(
                        args[0].object()?,
                        Self::property(&args[1])?,
                        args[2].clone(),
                        depth + 1,
                    )?;
                    return Ok(Value::Null);
                }
                _ => {}
            }
        }
        if owner == "System.Windows.DependencyPropertyChangedEventArgs" && name == "get_NewValue" {
            return match &args[0] {
                Value::Change(value) => Ok((**value).clone()),
                Value::Address(value) => {
                    if let Value::Change(value) = &*value.borrow() {
                        Ok((**value).clone())
                    } else {
                        Err("invalid property-change event address".into())
                    }
                }
                _ => Err("invalid property-change event".into()),
            };
        }
        if owner == "System.Windows.Media.Effects.PixelShader" && name == "set_UriSource" {
            // Store the URI on its shader object for the effect's later assignment.
            let object = args[0].object()?;
            self.objects[object]
                .fields
                .insert("UriSource".into(), args[1].clone());
            return Ok(Value::Null);
        }
        if owner == "System.Windows.Data.Binding" && name == "set_Source" {
            return Ok(Value::Null);
        }
        if owner == "System.Windows.Data.BindingOperations" && name == "SetBinding" {
            // Resolve the avatar asset path through the control's declared binding.
            let object = args[0].object()?;
            let property = Self::property(&args[1])?;
            let binding = args[2].object()?;
            let path = self.objects[binding]
                .fields
                .get("path")
                .ok_or("binding has no property path")?
                .text()?;
            let source = match path {
                "Avatar.ShaderData.AssetOne" => Source::AvatarOne,
                "Avatar.ShaderData.AssetTwo" => Source::AvatarTwo,
                _ => return Err(format!("unsupported avatar image binding {path}").into()),
            };
            self.set_value(object, property, Value::Image(source), depth + 1)?;
            return Ok(Value::Null);
        }
        if name == "set_Effect" && owner == "System.Windows.UIElement" {
            return Ok(Value::Null);
        }
        if matches!(
            name,
            "StartAnimations"
                | "StartMouseAnimation"
                | "StartTimeAnimation"
                | "StopAnimations"
                | "StopMouseAnimation"
                | "StopTimeAnimation"
                | "HandleMouseEnter"
                | "HandleMouseLeave"
        ) {
            return Ok(Value::Null);
        }
        if name == "GetPremiumEffect" && owner == "WotC.MtGO.Client.Model.Core.DigitalMagicObject" {
            return Ok(Value::Integer(i64::from(self.effect)));
        }
        if name == "get_Avatar"
            && (owner.ends_with("CardDefinition") || owner.ends_with("AvatarControl"))
        {
            return Ok(Value::Avatar);
        }
        if name == "get_FrameStyle" && owner.ends_with("CardDefinition") {
            return Ok(Value::Unknown);
        }
        if name == "SupportsNewFrames" && owner.ends_with(".CardFrameStyleHelpers") {
            return Ok(Value::Integer(i64::from(self.frame_support)));
        }
        if name == "get_ShaderData" && matches!(args.first(), Some(Value::Avatar)) {
            return Ok(Value::ShaderData);
        }
        if name == "get_EffectType" && matches!(args.first(), Some(Value::ShaderData)) {
            return Ok(Value::Integer(self.effect_type));
        }
        if name == "get_IsMini" && owner.ends_with(".AvatarControl") {
            return Ok(Value::Integer(0));
        }
        if owner == "System.Guid" && name == "NewGuid" {
            return Ok(Value::Unknown);
        }
        if owner == "System.DateTime" {
            return Ok(Value::Unknown);
        }
        if owner == "System.TimeSpan" && matches!(name, "FromSeconds" | "FromMilliseconds") {
            return Ok(Value::Unknown);
        }
        if name == "set_ShowAnimation" {
            return Ok(Value::Null);
        }
        if owner.starts_with("System.Windows.Media.Animation.")
            || owner.ends_with("IntervalDoubleAnimation")
        {
            return Ok(Value::Unknown);
        }
        if self.ty(owner).is_some() {
            // Resolve virtual calls against the modeled receiver before reading its IL.
            if !method.has_this {
                self.initialize(owner, depth + 1)?;
            }
            let runtime_owner = if virtual_call && method.has_this {
                args.first()
                    .and_then(|value| value.object().ok())
                    .map(|index| self.objects[index].ty.clone())
                    .unwrap_or_else(|| owner.into())
            } else {
                owner.into()
            };
            let implementation = self.method(&runtime_owner, name, method.parameter_count)?;
            return self.run(&implementation, args, depth + 1);
        }
        Err(format!(
            "unsupported binding call {owner}::{name}/{}",
            method.parameter_count
        )
        .into())
    }
}
