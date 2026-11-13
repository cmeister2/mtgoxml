//! Decode captured managed assemblies into the IL facts used by animation extraction.
//!
//! The decoder reads PE metadata and method bodies without executing assemblies.
//! Member references carry their actual signatures, and branch operands retain
//! absolute IL byte offsets. Short encodings are normalized to the corresponding
//! semantic opcode so consumers do not depend on compiler instruction choices.

use std::collections::BTreeMap;

/// Canonical CLI operation kinds reused directly from the structured decoder.
pub(crate) use cildec::il::OpCode;
use cildec::tables::{FieldRow, MethodDefRow, TypeDefRow};
use cildec::{FieldSig, Metadata, MethodBody, MethodSig, Names, PeImage, Rid, TableId, Token};

use super::super::Result;

/// Bound repeated metadata references and instruction allocation independently of file size.
const MAX_MEMBERS: u32 = 100_000;
/// Bound aggregate decoded instructions even when method bodies overlap in the image.
const MAX_INSTRUCTIONS: usize = 1_000_000;

/// Owned metadata and IL from one captured managed assembly.
#[derive(Debug, Clone)]
pub(crate) struct Assembly {
    /// Assembly identity retained for extraction diagnostics.
    pub name: String,
    /// Fully qualified type names, including `/` for nested types.
    pub types: BTreeMap<String, Type>,
}

/// A managed type together with its declared members.
#[derive(Debug, Clone)]
pub(crate) struct Type {
    /// Base type used to follow inherited shader properties.
    pub base: Option<String>,
    /// Declared fields, including enum literals and static initialization targets.
    pub fields: BTreeMap<String, Field>,
    /// Declared methods in metadata order, retaining overloads separately.
    pub methods: Vec<Method>,
}

/// One declared field and any literal metadata value.
#[derive(Debug, Clone)]
pub(crate) struct Field {
    /// Metadata field name.
    pub name: String,
    /// Resolved field type using IL primitive names.
    pub ty: String,
    /// Literal value from the metadata Constant table; `.cctor` values remain IL.
    pub constant: Option<Constant>,
}

/// Literal metadata values needed for effect codes and animation defaults.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Constant {
    /// Integral literals; unsigned 64-bit values retain their original bit pattern.
    Integer(i64),
    /// Single- or double-precision numeric literals widened to `f64`.
    Float(f64),
    /// An original UTF-16 metadata string.
    String(String),
    /// A null reference literal.
    Null,
}

/// One method body and the signature needed for bounded interpretation.
#[derive(Debug, Clone)]
pub(crate) struct Method {
    /// Fully qualified declaring type.
    pub owner: String,
    /// Metadata method name, including `.ctor` and `.cctor`.
    pub name: String,
    /// Whether callers omit the receiver argument.
    pub is_static: bool,
    /// Number of explicit arguments, excluding the receiver.
    pub parameter_count: usize,
    /// Whether normal return pushes a value for the caller.
    pub returns_value: bool,
    /// Whether the method requires exception semantics beyond straight-line IL.
    pub has_exception_handlers: bool,
    /// Instructions retaining byte offsets for branches and diagnostics.
    pub body: Vec<Instruction>,
}

/// One normalized IL operation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Instruction {
    /// Absolute byte offset within the original method body.
    pub offset: u32,
    /// Semantic CLI operation, with macro and short encodings normalized.
    pub opcode: OpCode,
    /// Decoded operand with resolved metadata references.
    pub operand: Operand,
}

/// Operands whose semantics are consumed by extraction and animation patterns.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Operand {
    /// An instruction with no explicit or implied operand.
    None,
    /// A signed integer operand, including widened short constants.
    Integer(i64),
    /// A floating-point operand widened to `f64`.
    Float(f64),
    /// A resolved user string.
    String(String),
    /// A call, constructor, or function-pointer target.
    Method(MethodRef),
    /// An instance or static field target.
    Field(FieldRef),
    /// A resolved managed type.
    Type(String),
    /// An absolute byte offset for a branch target.
    Branch(u32),
    /// Absolute byte offsets for a switch's targets in original order.
    Switch(Vec<u32>),
    /// A local variable or argument slot, including short and macro encodings.
    Variable(u16),
    /// Signature of an indirect call, retained for an explicit unsupported-operation decision.
    Signature(MethodRef),
}

/// Signature-aware method reference resolved from a metadata token.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MethodRef {
    /// Fully qualified declaring type.
    pub owner: String,
    /// Method name without decompiler formatting or assembly qualifications.
    pub name: String,
    /// Number of explicit arguments, excluding the receiver.
    pub parameter_count: usize,
    /// Resolved explicit parameter types, retaining overload distinctions.
    pub parameter_types: Vec<String>,
    /// Whether ordinary calls consume an instance receiver.
    pub has_this: bool,
    /// Whether the declared return type is non-void; constructors still declare void.
    pub returns_value: bool,
}

/// A field reference resolved from a declaration or external member reference.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FieldRef {
    /// Fully qualified declaring type.
    pub owner: String,
    /// Original field name.
    pub name: String,
    /// Resolved field type using IL primitive names.
    pub ty: String,
}

impl Assembly {
    /// Decode types, signatures, literal constants, and IL directly from managed PE bytes.
    ///
    /// # Errors
    /// Returns an error for malformed metadata, invalid method bodies or references,
    /// unsupported operand encodings, duplicate declarations, or extraction size limits.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let image = PeImage::parse(bytes)?;
        let metadata = image.metadata()?;
        let tables = metadata.tables();
        for table in [TableId::TypeDef, TableId::Field, TableId::MethodDef] {
            if tables.row_count(table) > MAX_MEMBERS {
                return Err(format!("IL extraction exceeds the {} limit", table.name()).into());
            }
        }
        let names = Names::new(&metadata);
        let mut types = BTreeMap::new();
        for (rid, row) in tables.iter::<TypeDefRow>() {
            // Establish nominal owners before resolving their fields and methods.
            let row = row?;
            let name = canonical_name(&names.type_def(Rid::new(rid))?);
            let base = (!row.extends.is_null())
                .then(|| {
                    names
                        .try_token(row.extends)
                        .map(|name| canonical_name(&name))
                })
                .transpose()?;
            let item = Type {
                base,
                fields: BTreeMap::new(),
                methods: Vec::new(),
            };
            if types.insert(name.clone(), item).is_some() {
                return Err(format!("duplicate IL type {name}").into());
            }
        }
        for (rid, row) in tables.iter::<FieldRow>() {
            // Keep literal metadata and field signatures with their declaring type.
            let row = row?;
            let owner = tables
                .type_of_field(Rid::new(rid))?
                .ok_or("IL field has no declaring type")?;
            let owner = canonical_name(&names.type_def(owner)?);
            let name = metadata.strings().str(row.name)?.to_owned();
            let signature = FieldSig::parse(metadata.blobs().get(row.signature)?)?;
            let constant = tables
                .constant_of(Token::new(TableId::Field, rid))?
                .map(|row| decode_constant(&metadata, row))
                .transpose()?;
            let field = Field {
                name: name.clone(),
                ty: canonical_name(&names.type_(&signature.type_)),
                constant,
            };
            let item = types.get_mut(&owner).ok_or("missing IL field owner")?;
            if item.fields.insert(name.clone(), field).is_some() {
                return Err(format!("duplicate IL field {owner}::{name}").into());
            }
        }
        let mut instruction_count = 0;
        for (rid, row) in tables.iter::<MethodDefRow>() {
            // Decode bodies only after resolving the exact call-stack signature.
            let row = row?;
            let reference = method_reference(&metadata, Token::new(TableId::MethodDef, rid))?;
            let body = MethodBody::from_image(&image, &row)?;
            let has_exception_handlers = body
                .as_ref()
                .is_some_and(|body| !body.exception_handlers.is_empty());
            let mut instructions = Vec::new();
            if let Some(body) = body {
                // Validate control-flow destinations against the complete decoded body.
                body.validate_handlers()?;
                for instruction in body.instructions() {
                    // Bound allocation across every method, including overlapping bodies.
                    instruction_count += 1;
                    if instruction_count > MAX_INSTRUCTIONS {
                        return Err("IL extraction exceeds the instruction limit".into());
                    }
                    instructions.push(decode_instruction(&metadata, instruction?)?);
                }
                let offsets: std::collections::BTreeSet<_> = instructions
                    .iter()
                    .map(|instruction| instruction.offset)
                    .collect();
                for instruction in &instructions {
                    let targets = match &instruction.operand {
                        Operand::Branch(target) => std::slice::from_ref(target),
                        Operand::Switch(targets) => targets.as_slice(),
                        _ => continue,
                    };
                    if targets.iter().any(|target| !offsets.contains(target)) {
                        return Err(format!(
                            "invalid IL branch in {}::{} at {}",
                            reference.owner, reference.name, instruction.offset
                        )
                        .into());
                    }
                }
            }
            let method = Method {
                owner: reference.owner.clone(),
                name: reference.name,
                is_static: row.flags & 0x0010 != 0,
                parameter_count: reference.parameter_count,
                returns_value: reference.returns_value,
                has_exception_handlers,
                body: instructions,
            };
            types
                .get_mut(&reference.owner)
                .ok_or("missing IL method owner")?
                .methods
                .push(method);
        }
        let name = if tables.row_count(TableId::Assembly) > 0 {
            metadata.strings().str(tables.assembly(1)?.name)?.to_owned()
        } else {
            metadata.strings().str(tables.module(1)?.name)?.to_owned()
        };
        Ok(Self { name, types })
    }
}

/// Convert CLI operand types and references without parsing decompiled C# or IL text.
fn decode_instruction(
    metadata: &Metadata<'_>,
    instruction: cildec::Instruction,
) -> Result<Instruction> {
    let operand = match instruction.operand {
        cildec::Operand::None => Operand::None,
        cildec::Operand::I8(value) => Operand::Integer(i64::from(value)),
        cildec::Operand::I32(value) => Operand::Integer(i64::from(value)),
        cildec::Operand::I64(value) => Operand::Integer(value),
        cildec::Operand::R32(value) => Operand::Float(f64::from(value)),
        cildec::Operand::R64(value) => Operand::Float(value),
        cildec::Operand::Var(value) => Operand::Variable(value),
        cildec::Operand::BranchTarget(target) => Operand::Branch(target),
        cildec::Operand::Switch(targets) => Operand::Switch(targets),
        cildec::Operand::String(token) => {
            let string = metadata.user_strings().get(token)?;
            Operand::String(decode_utf16(string.bytes)?)
        }
        cildec::Operand::Method(token) => Operand::Method(method_reference(metadata, token)?),
        cildec::Operand::Field(token) => Operand::Field(field_reference(metadata, token)?),
        cildec::Operand::Type(token) => {
            Operand::Type(canonical_name(&Names::new(metadata).try_token(token)?))
        }
        cildec::Operand::Tok(token) => match token.table() {
            Some(TableId::MethodDef | TableId::MethodSpec) => {
                Operand::Method(method_reference(metadata, token)?)
            }
            Some(TableId::Field) => Operand::Field(field_reference(metadata, token)?),
            Some(TableId::MemberRef) => {
                let row = metadata.tables().member_ref(token.rid())?;
                if metadata.blobs().get(row.signature)?.first() == Some(&0x06) {
                    Operand::Field(field_reference(metadata, token)?)
                } else {
                    Operand::Method(method_reference(metadata, token)?)
                }
            }
            _ => Operand::Type(canonical_name(&Names::new(metadata).try_token(token)?)),
        },
        cildec::Operand::Sig(token) => {
            // Keep indirect signatures inspectable without allowing their execution.
            let row = metadata.tables().stand_alone_sig(token.rid())?;
            let signature = MethodSig::parse(metadata.blobs().get(row.signature)?)?;
            Operand::Signature(MethodRef {
                owner: String::new(),
                name: "calli".to_owned(),
                parameter_count: signature.param_count(),
                parameter_types: signature
                    .params
                    .iter()
                    .chain(&signature.vararg_params)
                    .map(|parameter| canonical_name(&Names::new(metadata).type_(&parameter.type_)))
                    .collect(),
                has_this: signature.has_this,
                returns_value: signature.has_return(),
            })
        }
        _ => return Err("unrecognized IL operand".into()),
    };
    Ok(Instruction {
        offset: instruction.offset,
        opcode: instruction.opcode.canonical(),
        operand,
    })
}

/// Resolve a method's declaring type and exact call-stack signature.
fn method_reference(metadata: &Metadata<'_>, token: Token) -> Result<MethodRef> {
    let tables = metadata.tables();
    if token.table() == Some(TableId::MethodSpec) {
        return method_reference(metadata, tables.method_spec(token.rid())?.method);
    }
    let (owner, name, signature) = match token.table() {
        Some(TableId::MethodDef) => {
            // Follow declaration ownership rather than inferring it from display text.
            let row = tables.method_def(token.rid())?;
            let owner = tables
                .type_of_method(Rid::new(token.rid()))?
                .ok_or("IL method reference has no declaring type")?;
            (
                Names::new(metadata).type_def(owner)?,
                row.name,
                row.signature,
            )
        }
        Some(TableId::MemberRef) => {
            let row = tables.member_ref(token.rid())?;
            (
                Names::new(metadata).try_token(row.class)?,
                row.name,
                row.signature,
            )
        }
        _ => return Err("IL method reference has an invalid token kind".into()),
    };
    let signature = MethodSig::parse(metadata.blobs().get(signature)?)?;
    let names = Names::new(metadata);
    let parameter_types = signature
        .params
        .iter()
        .chain(&signature.vararg_params)
        .map(|parameter| {
            let name = canonical_name(&names.type_(&parameter.type_));
            if parameter.by_ref {
                format!("{name}&")
            } else {
                name
            }
        })
        .collect();
    Ok(MethodRef {
        owner: canonical_name(&owner),
        name: metadata.strings().str(name)?.to_owned(),
        parameter_count: signature.param_count(),
        parameter_types,
        has_this: signature.has_this,
        returns_value: signature.has_return(),
    })
}

/// Resolve a field's owner and type for static initialization and instance dataflow.
fn field_reference(metadata: &Metadata<'_>, token: Token) -> Result<FieldRef> {
    let tables = metadata.tables();
    let names = Names::new(metadata);
    let (owner, name, signature) = match token.table() {
        Some(TableId::Field) => {
            // Resolve a declaration's owner and signature from its metadata row.
            let row = tables.field(token.rid())?;
            let owner = tables
                .type_of_field(Rid::new(token.rid()))?
                .ok_or("IL field reference has no declaring type")?;
            (names.type_def(owner)?, row.name, row.signature)
        }
        Some(TableId::MemberRef) => {
            let row = tables.member_ref(token.rid())?;
            (names.try_token(row.class)?, row.name, row.signature)
        }
        _ => return Err("IL field reference has an invalid token kind".into()),
    };
    let signature = FieldSig::parse(metadata.blobs().get(signature)?)?;
    Ok(FieldRef {
        owner: canonical_name(&owner),
        name: metadata.strings().str(name)?.to_owned(),
        ty: canonical_name(&names.type_(&signature.type_)),
    })
}

/// Decode the limited literal kinds allowed by the CLI Constant table.
fn decode_constant(metadata: &Metadata<'_>, row: cildec::tables::ConstantRow) -> Result<Constant> {
    let bytes = metadata.blobs().get(row.value)?;
    let integer = match row.type_raw & 0xff {
        0x02 | 0x05 => Some(i64::from(u8::from_le_bytes(bytes.try_into()?))),
        0x03 | 0x07 => Some(i64::from(u16::from_le_bytes(bytes.try_into()?))),
        0x04 => Some(i64::from(i8::from_le_bytes(bytes.try_into()?))),
        0x06 => Some(i64::from(i16::from_le_bytes(bytes.try_into()?))),
        0x08 => Some(i64::from(i32::from_le_bytes(bytes.try_into()?))),
        0x09 => Some(i64::from(u32::from_le_bytes(bytes.try_into()?))),
        0x0a | 0x0b => Some(i64::from_le_bytes(bytes.try_into()?)),
        _ => None,
    };
    if let Some(value) = integer {
        return Ok(Constant::Integer(value));
    }
    Ok(match row.type_raw & 0xff {
        0x0c => Constant::Float(f64::from(f32::from_le_bytes(bytes.try_into()?))),
        0x0d => Constant::Float(f64::from_le_bytes(bytes.try_into()?)),
        0x0e => Constant::String(decode_utf16(bytes)?),
        0x12 if bytes == [0, 0, 0, 0] => Constant::Null,
        _ => return Err("unsupported IL metadata constant".into()),
    })
}

/// Preserve exact Unicode strings while rejecting odd lengths and invalid UTF-16.
fn decode_utf16(bytes: &[u8]) -> Result<String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("odd-length IL string".into());
    }
    let units: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    Ok(String::from_utf16(&units)?)
}

/// Remove display-only assembly and class prefixes while preserving actual type names.
fn canonical_name(name: &str) -> String {
    let name = name
        .strip_prefix("class ")
        .or_else(|| name.strip_prefix("valuetype "))
        .unwrap_or(name);
    match name.strip_prefix('[').and_then(|name| name.split_once(']')) {
        Some((_, rest)) => rest.to_owned(),
        None => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A self-authored metadata image supplies exact tokens without a proprietary DLL fixture.
    fn metadata_fixture() -> Vec<u8> {
        let strings = b"\0Demo\0Example\0Effect\0Helper\0Caller\0".to_vec();
        let blobs = vec![
            0, 2, 0x06, 0x08, 5, 0, 2, 0x0d, 0x08, 0x0e, 3, 0, 0, 1, 4, 24, 0, 0, 0,
        ];
        let mut tables = Vec::new();
        tables.extend_from_slice(&0u32.to_le_bytes());
        tables.extend_from_slice(&[2, 0, 0, 1]);
        let valid = (1u64 << 2) | (1 << 4) | (1 << 6) | (1 << 11);
        tables.extend_from_slice(&valid.to_le_bytes());
        tables.extend_from_slice(&0u64.to_le_bytes());
        for count in [1u32, 1, 2, 1] {
            tables.extend_from_slice(&count.to_le_bytes());
        }
        tables.extend_from_slice(&1u32.to_le_bytes());
        for column in [1u16, 6, 0, 1, 1] {
            tables.extend_from_slice(&column.to_le_bytes());
        }
        for column in [0x8056u16, 14, 1] {
            tables.extend_from_slice(&column.to_le_bytes());
        }
        for (name, signature) in [(21u16, 4u16), (28, 10)] {
            tables.extend_from_slice(&0u32.to_le_bytes());
            for column in [0u16, 0x0016, name, signature, 1] {
                tables.extend_from_slice(&column.to_le_bytes());
            }
        }
        for column in [8u16, 4, 14] {
            tables.extend_from_slice(&column.to_le_bytes());
        }
        let streams = [("#~", tables), ("#Strings", strings), ("#Blob", blobs)];
        let version = b"v4.0.30319\0\0";
        let mut metadata = b"BSJB".to_vec();
        metadata.extend_from_slice(&1u16.to_le_bytes());
        metadata.extend_from_slice(&1u16.to_le_bytes());
        metadata.extend_from_slice(&0u32.to_le_bytes());
        metadata.extend_from_slice(&(version.len() as u32).to_le_bytes());
        metadata.extend_from_slice(version);
        metadata.extend_from_slice(&0u16.to_le_bytes());
        metadata.extend_from_slice(&(streams.len() as u16).to_le_bytes());
        let directory_size: usize = streams
            .iter()
            .map(|(name, _)| 8 + (name.len() + 1).div_ceil(4) * 4)
            .sum();
        let mut offset = metadata.len() + directory_size;
        for (name, bytes) in &streams {
            metadata.extend_from_slice(&(offset as u32).to_le_bytes());
            metadata.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            metadata.extend_from_slice(name.as_bytes());
            metadata.resize(
                metadata.len() + (name.len() + 1).div_ceil(4) * 4 - name.len(),
                0,
            );
            offset += bytes.len();
        }
        for (_, bytes) in streams {
            metadata.extend_from_slice(&bytes);
        }
        metadata
    }

    /// Call stack signatures and original branch positions survive normalization and bad tokens fail.
    #[test]
    fn resolves_call_signatures_constants_and_macro_operands() -> Result<()> {
        let bytes = metadata_fixture();
        let metadata = Metadata::parse(&bytes)?;
        let call = method_reference(&metadata, Token::new(TableId::MethodDef, 1))?;
        assert_eq!(call.owner, "Example.Demo");
        assert_eq!(call.name, "Helper");
        assert_eq!(call.parameter_types, ["int32", "string"]);
        assert_eq!(call.parameter_count, 2);
        assert!(call.returns_value);
        assert!(!call.has_this);
        let field = field_reference(&metadata, Token::new(TableId::Field, 1))?;
        assert_eq!(field.owner, "Example.Demo");
        assert_eq!(field.name, "Effect");
        let constant = metadata
            .tables()
            .constant_of(Token::new(TableId::Field, 1))?
            .unwrap();
        assert_eq!(decode_constant(&metadata, constant)?, Constant::Integer(24));

        let code = [0x02, 0x1f, 0xff, 0x2b, 0x05, 0x28, 1, 0, 0, 6, 0x2a];
        let decoded = cildec::InstructionIter::new(&code)
            .map(|instruction| decode_instruction(&metadata, instruction?))
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(
            (decoded[0].opcode, &decoded[0].operand),
            (OpCode::Ldarg, &Operand::Variable(0))
        );
        assert_eq!(
            (decoded[1].opcode, &decoded[1].operand),
            (OpCode::LdcI4, &Operand::Integer(-1))
        );
        assert_eq!(
            (decoded[2].opcode, &decoded[2].operand),
            (OpCode::Br, &Operand::Branch(10))
        );
        assert_eq!(decoded[3].offset, 5);
        assert_eq!(decoded[3].operand, Operand::Method(call));
        assert!(method_reference(&metadata, Token::new(TableId::MethodDef, 999)).is_err());
        assert!(
            cildec::InstructionIter::new(&[0x28, 1])
                .next()
                .unwrap()
                .is_err()
        );
        Ok(())
    }
}
