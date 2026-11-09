//! Functional extraction and playback regressions over self-authored and optional captured IL.

use super::super::il::Type;
use super::*;

/// Construct signature-aware framework call operands for self-authored IL.
fn call(owner: &str, name: &str, count: usize, instance: bool, returns: bool) -> Operand {
    Operand::Method(MethodRef {
        owner: owner.into(),
        name: name.into(),
        parameter_count: count,
        parameter_types: vec!["float64".into(); count],
        has_this: instance,
        returns_value: returns,
    })
}

/// Build a tiny synthetic assembly from decoded instruction facts, without client code.
fn assembly(body: Vec<(OpCode, Operand)>) -> Assembly {
    let owner = "Synthetic.Effect";
    let method = Method {
        owner: owner.into(),
        name: "Create".into(),
        is_static: true,
        parameter_count: 0,
        returns_value: true,
        has_exception_handlers: false,
        body: body
            .into_iter()
            .enumerate()
            .map(|(offset, (opcode, operand))| Instruction {
                offset: offset as u32,
                opcode,
                operand,
            })
            .collect(),
    };
    Assembly {
        name: "Synthetic".into(),
        types: BTreeMap::from([(
            owner.into(),
            Type {
                base: None,
                fields: BTreeMap::new(),
                methods: vec![method],
            },
        )]),
    }
}

/// Changed captured literals alter playback automatically, while unknown IL is rejected.
#[test]
fn decoded_keyframes_drive_playback_without_a_source_hash() -> Result<()> {
    let mut body = vec![(
        OpCode::Newobj,
        call(
            "System.Windows.Media.Animation.DoubleAnimationUsingKeyFrames",
            ".ctor",
            0,
            true,
            false,
        ),
    )];
    for (value, seconds) in [(0.0, 0.0), (1.0, 3.0)] {
        body.extend([
            (OpCode::Dup, Operand::None),
            (
                OpCode::Callvirt,
                call(
                    "System.Windows.Media.Animation.DoubleAnimationUsingKeyFrames",
                    "get_KeyFrames",
                    0,
                    true,
                    true,
                ),
            ),
            (OpCode::LdcR8, Operand::Float(value)),
            (OpCode::LdcR8, Operand::Float(seconds)),
            (
                OpCode::Call,
                call("System.TimeSpan", "FromSeconds", 1, false, true),
            ),
            (
                OpCode::Call,
                call(
                    "System.Windows.Media.Animation.KeyTime",
                    "FromTimeSpan",
                    1,
                    false,
                    true,
                ),
            ),
            (
                OpCode::Newobj,
                call(
                    "System.Windows.Media.Animation.LinearDoubleKeyFrame",
                    ".ctor",
                    2,
                    true,
                    false,
                ),
            ),
            (
                OpCode::Callvirt,
                call(
                    "System.Windows.Media.Animation.DoubleKeyFrameCollection",
                    "Add",
                    1,
                    true,
                    true,
                ),
            ),
            (OpCode::Pop, Operand::None),
        ]);
    }
    body.push((OpCode::Ret, Operand::None));
    let mut source = assembly(body);
    let timeline = extract(&source, "Synthetic.Effect", "Create", &[])?;
    assert_eq!(timeline.period_ms()?, 3000);
    assert_eq!(timeline.sample(1.5)?, 0.5);
    source.types.get_mut("Synthetic.Effect").unwrap().methods[0].body[13].operand =
        Operand::Float(6.0);
    let changed = extract(&source, "Synthetic.Effect", "Create", &[])?;
    assert_eq!(changed.period_ms()?, 6000);
    assert_eq!(changed.sample(1.5)?, 0.25);
    source.types.get_mut("Synthetic.Effect").unwrap().methods[0].body[0].opcode = OpCode::Calli;
    let failure = extract(&source, "Synthetic.Effect", "Create", &[]).unwrap_err();
    assert!(failure.to_string().contains("instruction calli"));
    let looping = assembly(vec![(OpCode::Br, Operand::Branch(0))]);
    assert!(
        extract(&looping, "Synthetic.Effect", "Create", &[])
            .unwrap_err()
            .to_string()
            .contains("limit exceeded")
    );
    let integer_duration = assembly(vec![
        (OpCode::LdcR8, Operand::Float(0.0)),
        (OpCode::LdcR8, Operand::Float(1.0)),
        (OpCode::LdcI4, Operand::Integer(3)),
        (OpCode::LdcI4, Operand::Integer(2)),
        (OpCode::Div, Operand::None),
        (OpCode::ConvR8, Operand::None),
        (
            OpCode::Call,
            call("System.TimeSpan", "FromSeconds", 1, false, true),
        ),
        (
            OpCode::Newobj,
            call("System.Windows.Duration", ".ctor", 1, true, false),
        ),
        (
            OpCode::Newobj,
            call(
                "System.Windows.Media.Animation.DoubleAnimation",
                ".ctor",
                3,
                true,
                false,
            ),
        ),
        (OpCode::Ret, Operand::None),
    ]);
    let integer = extract(&integer_duration, "Synthetic.Effect", "Create", &[])?;
    assert_eq!(integer.period_ms()?, 1000);
    assert_eq!(integer.sample(0.5)?, 0.5);
    Ok(())
}

/// WPF uses a target property's captured base value when an animation omits From.
#[test]
fn begin_animation_resolves_an_implicit_property_origin() -> Result<()> {
    let property = "Synthetic.Effect::ValueProperty";
    let mut source = assembly(vec![
        (OpCode::Ldarg, Operand::Variable(0)),
        (
            OpCode::Ldsfld,
            Operand::Field(FieldRef {
                owner: "Synthetic.Effect".into(),
                name: "ValueProperty".into(),
                ty: "System.Windows.DependencyProperty".into(),
            }),
        ),
        (OpCode::LdcR8, Operand::Float(10.0)),
        (OpCode::LdcR8, Operand::Float(1.0)),
        (
            OpCode::Call,
            call("System.TimeSpan", "FromSeconds", 1, false, true),
        ),
        (
            OpCode::Newobj,
            call("System.Windows.Duration", ".ctor", 1, true, false),
        ),
        (
            OpCode::Newobj,
            call(
                "System.Windows.Media.Animation.DoubleAnimation",
                ".ctor",
                2,
                true,
                false,
            ),
        ),
        (
            OpCode::Callvirt,
            call(
                "System.Windows.Media.Animation.Animatable",
                "BeginAnimation",
                2,
                true,
                false,
            ),
        ),
        (OpCode::Ret, Operand::None),
    ]);
    let method = &mut source.types.get_mut("Synthetic.Effect").unwrap().methods[0];
    method.name = "StartAnimations".into();
    method.is_static = false;
    method.parameter_count = 1;
    method.returns_value = false;
    let set = animations(
        &source,
        "Synthetic.Effect",
        &BTreeMap::from([(property.into(), 5.0)]),
    )?;
    assert_eq!(set.tracks[0].property, property);
    assert_eq!(set.tracks[0].timeline.sample(0.0)?, 5.0);
    assert_eq!(set.tracks[0].timeline.sample(0.5)?, 7.5);
    assert_eq!(set.tracks[0].timeline.sample(1.0)?, 10.0);
    let defaults = BTreeMap::from([(property.into(), 5.0)]);
    let mut evaluator = Evaluator::new(&source, "Synthetic.Effect", &defaults);
    let Operand::Method(constructor) = call(
        "System.Windows.Media.Animation.DoubleAnimation",
        ".ctor",
        0,
        true,
        false,
    ) else {
        unreachable!()
    };
    let value = evaluator.construct(&constructor, Vec::new())?;
    let Operand::Method(set_from) = call(
        "System.Windows.Media.Animation.DoubleAnimation",
        "set_From",
        1,
        true,
        false,
    ) else {
        unreachable!()
    };
    evaluator.call(
        &set_from,
        Some(value.clone()),
        vec![Value::Number(2.0)],
        0,
        false,
    )?;
    let Value::Timeline(builder) = value else {
        unreachable!()
    };
    let to_default = builder.borrow().finish(Some(5.0))?;
    assert_eq!(to_default.sample(0.5)?, 3.5);
    let method = &mut source.types.get_mut("Synthetic.Effect").unwrap().methods[0];
    let last = method.body.pop().unwrap().offset;
    for (index, (opcode, operand)) in [
        (OpCode::Ldarg, Operand::Variable(0)),
        (
            OpCode::Ldsfld,
            Operand::Field(FieldRef {
                owner: "Synthetic.Effect".into(),
                name: "ValueProperty".into(),
                ty: "System.Windows.DependencyProperty".into(),
            }),
        ),
        (OpCode::Ldnull, Operand::None),
        (
            OpCode::Callvirt,
            call(
                "System.Windows.Media.Animation.Animatable",
                "BeginAnimation",
                2,
                true,
                false,
            ),
        ),
        (OpCode::Ret, Operand::None),
    ]
    .into_iter()
    .enumerate()
    {
        method.body.push(Instruction {
            offset: last + index as u32,
            opcode,
            operand,
        });
    }
    let reset = animations(&source, "Synthetic.Effect", &defaults)?;
    assert_eq!(reset.tracks.len(), 1);
    assert_eq!(reset.tracks[0].timeline.sample(0.5)?, 5.0);
    Ok(())
}

/// Exercise actual metadata and animation bodies when a local verified capture is supplied.
#[test]
fn extracts_current_animation_patterns_when_capture_is_supplied() -> Result<()> {
    let Some(directory) = std::env::var_os("MTGOXML_TEST_IL_BUNDLE") else {
        return Ok(());
    };
    let assembly = Assembly::parse(&std::fs::read(
        std::path::Path::new(&directory).join("original/ShaderEffects.dll"),
    )?)?;
    let stop = extract_static(&assembly, "ShaderEffects.AnimationLibrary", "StopAnimation")?;
    assert_eq!(stop.sample(0.0)?, 0.0);
    let interval = extract(
        &assembly,
        "ShaderEffects.LiquidMetal",
        "NewDoubleAnimation",
        &[],
    )?;
    assert_eq!(interval.period_ms()?, 60_000);
    assert_eq!(interval.sample(30.0)?, 30.0);
    let pointer = extract(
        &assembly,
        "ShaderEffects.AnimationLibrary",
        "DiagonalAnimation_MouseX",
        &[0.0],
    )?;
    assert_eq!(pointer.period_ms()?, 34_000);
    assert_eq!(pointer.sample(6.0)?, 1.0);
    assert_eq!(pointer.sample(12.0)?, 0.0);
    let mut changed = assembly.clone();
    let method = changed
        .types
        .get_mut("ShaderEffects.IntervalDoubleAnimation")
        .unwrap()
        .methods
        .iter_mut()
        .find(|method| method.name == "GetCurrentValue")
        .unwrap();
    method
        .body
        .iter_mut()
        .find(|instruction| instruction.opcode == OpCode::Mul)
        .unwrap()
        .opcode = OpCode::Div;
    assert!(
        extract(
            &changed,
            "ShaderEffects.LiquidMetal",
            "NewDoubleAnimation",
            &[]
        )
        .unwrap_err()
        .to_string()
        .contains("custom interpolation")
    );
    let mut changed = assembly.clone();
    let method = changed
        .types
        .get_mut("ShaderEffects.IntervalDoubleAnimation")
        .unwrap()
        .methods
        .iter_mut()
        .find(|method| method.name == "GetCurrentValue")
        .unwrap();
    let branch = method
        .body
        .iter_mut()
        .find(|instruction| instruction.opcode == OpCode::Brtrue)
        .unwrap();
    branch.opcode = OpCode::Brfalse;
    assert!(
        extract(
            &changed,
            "ShaderEffects.LiquidMetal",
            "NewDoubleAnimation",
            &[]
        )
        .unwrap_err()
        .to_string()
        .contains("branch topology")
    );
    let method = changed
        .types
        .get_mut("ShaderEffects.IntervalDoubleAnimation")
        .unwrap()
        .methods
        .iter_mut()
        .find(|method| method.name == "GetCurrentValue")
        .unwrap();
    let branch = method
        .body
        .iter_mut()
        .find(|instruction| instruction.opcode == OpCode::Brfalse)
        .unwrap();
    branch.opcode = OpCode::Brtrue;
    branch.operand = Operand::Branch(0);
    assert!(
        extract(
            &changed,
            "ShaderEffects.LiquidMetal",
            "NewDoubleAnimation",
            &[]
        )
        .unwrap_err()
        .to_string()
        .contains("branch topology")
    );
    let mut changed = assembly.clone();
    let constructor = changed
        .types
        .get_mut("ShaderEffects.IntervalDoubleAnimation")
        .unwrap()
        .methods
        .iter_mut()
        .find(|method| method.name == ".ctor" && method.parameter_count == 0)
        .unwrap();
    let sentinel = constructor.body.iter_mut().find(|instruction| matches!(&instruction.operand, Operand::Field(field) if field.owner == "System.DateTime" && field.name == "MinValue")).unwrap();
    sentinel.opcode = OpCode::Call;
    sentinel.operand = call("System.DateTime", "get_Now", 0, false, true);
    assert!(
        extract(
            &changed,
            "ShaderEffects.LiquidMetal",
            "NewDoubleAnimation",
            &[]
        )
        .unwrap_err()
        .to_string()
        .contains("initialization")
    );
    Ok(())
}
