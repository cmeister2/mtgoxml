//! Self-authored IL proves that bindings come from declarations and unsupported calls fail.

use super::super::il::{FieldRef, Instruction, Method, MethodRef, OpCode, Operand, Type};
use super::*;

/// Build a signature-aware call operand without a managed runtime.
fn target(owner: &str, name: &str, count: usize, instance: bool, returns: bool) -> Operand {
    Operand::Method(MethodRef {
        owner: owner.into(),
        name: name.into(),
        parameter_count: count,
        parameter_types: vec!["object".into(); count],
        has_this: instance,
        returns_value: returns,
    })
}

/// Refer to one synthetic dependency-property field.
fn field(name: &str) -> Operand {
    Operand::Field(FieldRef {
        owner: "Fixture.Shader".into(),
        name: name.into(),
        ty: "System.Windows.DependencyProperty".into(),
    })
}

/// Assign instruction offsets so branches describe real boundaries in the fixture.
fn method(
    owner: &str,
    name: &str,
    is_static: bool,
    returns: bool,
    body: Vec<(OpCode, Operand)>,
) -> Method {
    Method {
        owner: owner.into(),
        name: name.into(),
        is_static,
        parameter_count: 0,
        returns_value: returns,
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
    }
}

/// A tiny effect uses configurable register numbers and an avatar constructor override.
fn fixture(sampler: i64, register: i64) -> Assembly {
    use OpCode::*;
    let shader = "System.Windows.Media.Effects.ShaderEffect";
    let property = "System.Windows.DependencyProperty";
    let ty = "System.Type";
    let mut initialization = Vec::new();
    for (name, slot) in [("Input", 0), ("Alt", sampler)] {
        initialization.extend([
            (Ldstr, Operand::String(name.into())),
            (Ldtoken, Operand::Type("Fixture.Shader".into())),
            (Call, target(ty, "GetTypeFromHandle", 1, false, true)),
            (LdcI4, Operand::Integer(slot)),
            (
                Call,
                target(shader, "RegisterPixelShaderSamplerProperty", 3, false, true),
            ),
            (Stsfld, field(&format!("{name}Property"))),
        ]);
    }
    initialization.extend([
        (Ldstr, Operand::String("Clock".into())),
        (Ldtoken, Operand::Type("System.Double".into())),
        (Call, target(ty, "GetTypeFromHandle", 1, false, true)),
        (Ldtoken, Operand::Type("Fixture.Shader".into())),
        (Call, target(ty, "GetTypeFromHandle", 1, false, true)),
        (LdcR8, Operand::Float(9.5)),
        (Box, Operand::Type("System.Double".into())),
        (LdcI4, Operand::Integer(register)),
        (
            Call,
            target(shader, "PixelShaderConstantCallback", 1, false, true),
        ),
        (
            Newobj,
            target("System.Windows.UIPropertyMetadata", ".ctor", 2, true, false),
        ),
        (Call, target(property, "Register", 4, false, true)),
        (Stsfld, field("ClockProperty")),
        (Ret, Operand::None),
    ]);
    let mut constructor = vec![
        (Ldarg, Operand::Variable(0)),
        (
            Newobj,
            target(
                "System.Windows.Media.Effects.PixelShader",
                ".ctor",
                0,
                true,
                false,
            ),
        ),
        (Dup, Operand::None),
        (
            Ldstr,
            Operand::String(
                "pack://application:,,,/Fixture;component/Effects/NewShader/NewShader.ps".into(),
            ),
        ),
        (Newobj, target("System.Uri", ".ctor", 1, true, false)),
        (
            Callvirt,
            target(
                "System.Windows.Media.Effects.PixelShader",
                "set_UriSource",
                1,
                true,
                false,
            ),
        ),
        (Call, target(shader, "set_PixelShader", 1, true, false)),
    ];
    for name in ["InputProperty", "AltProperty", "ClockProperty"] {
        constructor.extend([
            (Ldarg, Operand::Variable(0)),
            (Ldsfld, field(name)),
            (Call, target(shader, "UpdateShaderValue", 1, true, false)),
        ]);
    }
    constructor.push((Ret, Operand::None));
    let control = "WotC.MtGO.Client.Model.Core.Controls.AvatarControl";
    let effect_field = Operand::Field(FieldRef {
        owner: control.into(),
        name: "m_effect".into(),
        ty: shader.into(),
    });
    let application = vec![
        // A boxed enum zero is a nonnull reference, as in LiquidSilver's callback.
        (LdcI4, Operand::Integer(0)),
        (Box, Operand::Type("Fixture.ZeroEnum".into())),
        (Isinst, Operand::Type("Fixture.ZeroEnum".into())),
        (Brtrue, Operand::Branch(5)),
        (Ret, Operand::None),
        (Ldarg, Operand::Variable(0)),
        (Newobj, target("Fixture.Shader", ".ctor", 0, true, false)),
        (Stfld, effect_field.clone()),
        (Ldarg, Operand::Variable(0)),
        (Ldfld, effect_field.clone()),
        (Ldsfld, field("AltProperty")),
        (Ldstr, Operand::String("Avatar.ShaderData.AssetOne".into())),
        (
            Newobj,
            target("System.Windows.Data.Binding", ".ctor", 1, true, false),
        ),
        (
            Call,
            target(
                "System.Windows.Data.BindingOperations",
                "SetBinding",
                3,
                false,
                true,
            ),
        ),
        (Pop, Operand::None),
        (Ldarg, Operand::Variable(0)),
        (Ldfld, effect_field),
        (Ldsfld, field("ClockProperty")),
        (LdcR8, Operand::Float(3.25)),
        (Box, Operand::Type("System.Double".into())),
        (
            Call,
            target(
                "System.Windows.DependencyObject",
                "SetValue",
                2,
                true,
                false,
            ),
        ),
        (Ret, Operand::None),
    ];
    let definition = "WotC.MtGO.Client.Model.Core.CardDefinition";
    let getter = method(
        definition,
        "get_PremiumEffect",
        false,
        true,
        vec![
            (Ldarg, Operand::Variable(0)),
            (
                Call,
                target(
                    "WotC.MtGO.Client.Model.Core.DigitalMagicObject",
                    "GetPremiumEffect",
                    0,
                    true,
                    true,
                ),
            ),
            (Ret, Operand::None),
        ],
    );
    let types = [
        (
            definition.to_owned(),
            Type {
                base: None,
                fields: BTreeMap::new(),
                methods: vec![getter],
            },
        ),
        (
            control.to_owned(),
            Type {
                base: None,
                fields: BTreeMap::new(),
                methods: vec![method(
                    control,
                    "ApplyShaderEffect",
                    false,
                    false,
                    application,
                )],
            },
        ),
        (
            "Fixture.Shader".to_owned(),
            Type {
                base: Some(shader.into()),
                fields: BTreeMap::new(),
                methods: vec![
                    method("Fixture.Shader", ".cctor", true, false, initialization),
                    method("Fixture.Shader", ".ctor", false, false, constructor),
                ],
            },
        ),
    ]
    .into_iter()
    .collect();
    Assembly {
        name: "Fixture".into(),
        types,
    }
}

/// New resource names and changed slots are read from IL, including instance overrides.
#[test]
fn declarations_and_avatar_overrides_determine_bindings() -> Result<()> {
    for (sampler, register) in [(1, 4), (3, 7)] {
        let binding = extract(&[fixture(sampler, register)], 99)?;
        assert_eq!(
            binding.shader_resource,
            "decompiled/Fixture/effects/newshader/newshader.ps"
        );
        assert_eq!(
            binding.samplers,
            BTreeMap::from([(0, Source::Input), (sampler as u32, Source::AvatarOne)])
        );
        let clock = &binding.constants["Fixture.Shader::ClockProperty"];
        assert_eq!(clock.register, register as usize);
        assert_eq!(clock.default, [3.25, 1.0, 1.0, 1.0]);
        assert!(clock.priority > 0);
    }
    // Supported numeric IL keeps CLI integer division and conversion semantics.
    for (expression, expected) in [
        (
            vec![
                (OpCode::LdcR8, Operand::Float(1.9)),
                (OpCode::ConvI4, Operand::None),
                (OpCode::ConvR8, Operand::None),
            ],
            1.0,
        ),
        (
            vec![
                (OpCode::LdcI4, Operand::Integer(5)),
                (OpCode::LdcI4, Operand::Integer(2)),
                (OpCode::Div, Operand::None),
                (OpCode::ConvR8, Operand::None),
            ],
            2.0,
        ),
    ] {
        let mut assembly = fixture(1, 4);
        let body = &mut assembly
            .types
            .get_mut("WotC.MtGO.Client.Model.Core.Controls.AvatarControl")
            .unwrap()
            .methods[0]
            .body;
        let index = body
            .iter()
            .position(|instruction| instruction.operand == Operand::Float(3.25))
            .unwrap();
        body.splice(
            index..=index,
            expression.into_iter().map(|(opcode, operand)| Instruction {
                offset: 0,
                opcode,
                operand,
            }),
        );
        for (offset, instruction) in body.iter_mut().enumerate() {
            instruction.offset = offset as u32;
        }
        let binding = extract(&[assembly], 99)?;
        assert_eq!(
            binding.constants["Fixture.Shader::ClockProperty"].default[0],
            expected
        );
    }
    Ok(())
}

/// An unfamiliar runtime call cannot be guessed or executed while reading bindings.
#[test]
fn unfamiliar_client_calls_report_unsupported() {
    let mut assembly = fixture(1, 4);
    let constructor = assembly
        .types
        .get_mut("Fixture.Shader")
        .unwrap()
        .methods
        .iter_mut()
        .find(|method| method.name == ".ctor")
        .unwrap();
    constructor.body.insert(
        0,
        Instruction {
            offset: 1000,
            opcode: OpCode::Call,
            operand: target("System.IO.File", "ReadAllBytes", 0, false, true),
        },
    );
    let error = extract(&[assembly], 99).unwrap_err().to_string();
    assert!(
        error.contains("unsupported binding call System.IO.File::ReadAllBytes"),
        "{error}"
    );
    // A closed instance delegate must retain its target; this recognizer rejects it.
    let mut assembly = fixture(1, 4);
    let constructor = assembly
        .types
        .get_mut("Fixture.Shader")
        .unwrap()
        .methods
        .iter_mut()
        .find(|method| method.name == ".ctor")
        .unwrap();
    constructor.body.splice(
        0..0,
        [
            (OpCode::Ldarg, Operand::Variable(0)),
            (
                OpCode::Ldftn,
                target("Fixture.Shader", "OnChanged", 2, true, false),
            ),
            (
                OpCode::Newobj,
                target(
                    "System.Windows.PropertyChangedCallback",
                    ".ctor",
                    2,
                    true,
                    false,
                ),
            ),
            (OpCode::Pop, Operand::None),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (opcode, operand))| Instruction {
            offset: 1000 + index as u32,
            opcode,
            operand,
        }),
    );
    let error = extract(&[assembly], 99).unwrap_err().to_string();
    assert!(
        error.contains("unsupported bound or nonstatic property-change callback"),
        "{error}"
    );
}
