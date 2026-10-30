#![doc = include_str!("../docs/card-styles.md")]

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use roxmltree::Node;

pub use crate::dataxml::{Error, ParseOptions};

/// Named, unresolved declarations from one `CardStyle.xml` document.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CardStyles {
    /// Styles keyed by their exact case-sensitive `Name` attribute.
    pub styles: BTreeMap<String, CardStyle>,
}

/// One source style, including its optional `CloneFrom` reference.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CardStyle {
    /// Exact source name, without case folding or whitespace normalization.
    pub name: String,
    /// Another named style whose declared properties are inherited first.
    pub clone_from: Option<String>,
    /// Properties declared directly by this style rather than by its ancestors.
    pub properties: StyleProperties,
}

/// A requested style with every available clone ancestor applied.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ResolvedStyle {
    /// Name of the requested style, retaining its exact source spelling.
    pub name: String,
    /// Effective properties after applying declarations from base to child.
    pub properties: StyleProperties,
}

/// Optional declarations whose absence means inheritance rather than a default.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StyleProperties {
    /// Opaque `Frame` name, including literal values such as `none`.
    pub frame: Option<String>,
    /// Opaque `WatermarkEffect` name.
    pub watermark_effect: Option<String>,
    /// Opaque `Model` name.
    pub model: Option<String>,
    /// Exact artwork filename suffix, without inferred extensions or URLs.
    pub art_suffix: Option<String>,
    /// Explicit watermark-on-nonfoil choice; missing is distinct from false.
    pub use_watermark_effect_on_non_foils: Option<bool>,
    /// Individually inheritable texture names for the power/toughness box.
    pub pt_box: BoxTextures,
    /// Individually inheritable texture names for the casting-cost box.
    pub cc_box: BoxTextures,
    /// Logical source drawing size, without assumptions about screen pixels.
    pub reference_size: Option<Size>,
    /// Complete rectangles keyed by their exact source `rc...` element names.
    pub rectangles: BTreeMap<String, Rectangle>,
    /// Text declarations keyed by their exact `TextType` values.
    pub text_elements: BTreeMap<String, TextElement>,
}

/// Independent named textures making up a three-part box.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BoxTextures {
    /// Source name of the left texture.
    pub left: Option<String>,
    /// Source name of the right texture.
    pub right: Option<String>,
    /// Source name of the middle texture.
    pub middle: Option<String>,
}

/// Logical reference size declared by a style.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Size {
    /// Finite nonnegative width; zero remains a meaningful source value.
    pub width: f64,
    /// Finite nonnegative height; zero remains a meaningful source value.
    pub height: f64,
}

/// A complete logical rectangle, preserving negative positions and zero sizes.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Rectangle {
    /// Finite horizontal origin, which may be negative.
    pub x: f64,
    /// Finite vertical origin, which may be negative.
    pub y: f64,
    /// Finite nonnegative width.
    pub width: f64,
    /// Finite nonnegative height.
    pub height: f64,
}

/// One complete `TextElementEntry`, without invented font or formatting defaults.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TextElement {
    /// Exact `TextType` identifier identifying this entry within the style.
    pub text_type: String,
    /// Opaque `Font` token, such as `CARD_TITLE`.
    pub font: String,
    /// Opaque `ForeColor` name, such as `Bone`.
    pub fore_color: String,
    /// Optional literal `Style` value, without substituting `Normal`.
    pub font_style: Option<String>,
    /// Complete X/Y/W/H bounds, or no bounds when all four are absent.
    pub bounds: Option<Rectangle>,
}

impl CardStyles {
    /// Parse already decoded XML using the default byte and node limits.
    ///
    /// # Errors
    /// Returns XML or size-limit errors, unsupported source structure, missing
    /// required values, invalid geometry, or duplicate declarations. Missing
    /// clone bases and cycles are checked by [`Self::resolve`], not by parsing.
    pub fn parse(xml: &str) -> Result<Self, Error> {
        Self::parse_with_options(xml, ParseOptions::default())
    }

    /// Parse already decoded XML under caller-selected input and node limits.
    ///
    /// # Errors
    /// Returns the parsing errors described by [`Self::parse`].
    pub fn parse_with_options(xml: &str, options: ParseOptions) -> Result<Self, Error> {
        let document = options.document(xml)?;
        let root = document.root_element();
        require_name(root, "AnimCache")?;
        check_attributes(root, &[])?;
        let mut styles = BTreeMap::new();
        for node in element_children(root)? {
            // Retain incomplete clone graphs while enforcing unambiguous declarations.
            let style = parse_style(node)?;
            let name = style.name.clone();
            if styles.insert(name.clone(), style).is_some() {
                return Err(Error::Duplicate {
                    kind: "style",
                    key: name,
                });
            }
        }
        Ok(Self { styles })
    }

    /// Decode and parse source bytes using the default limits.
    ///
    /// # Errors
    /// Returns decoding failures or the errors described by [`Self::parse`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_bytes_with_options(bytes, ParseOptions::default())
    }

    /// Decode and parse source bytes under caller-selected limits.
    ///
    /// # Errors
    /// Returns decoding failures or the errors described by [`Self::parse`].
    pub fn from_bytes_with_options(bytes: &[u8], options: ParseOptions) -> Result<Self, Error> {
        let xml = options.decode(bytes)?;
        Self::parse_with_options(&xml, options)
    }

    /// Read a bounded source document and parse it using the default limits.
    ///
    /// # Errors
    /// Returns read failures or the errors described by [`Self::from_bytes`].
    pub fn from_reader(reader: impl Read) -> Result<Self, Error> {
        Self::from_reader_with_options(reader, ParseOptions::default())
    }

    /// Read and parse a source document under caller-selected limits.
    ///
    /// # Errors
    /// Returns read failures or the errors described by [`Self::from_bytes`].
    pub fn from_reader_with_options(
        reader: impl Read,
        options: ParseOptions,
    ) -> Result<Self, Error> {
        let bytes = options.read(reader)?;
        Self::from_bytes_with_options(&bytes, options)
    }

    /// Look up an unresolved declaration by its exact case-sensitive name.
    pub fn get(&self, name: &str) -> Option<&CardStyle> {
        self.styles.get(name)
    }

    /// Resolve an explicit clone chain without guessing missing bases or defaults.
    ///
    /// Present scalar properties and individual box textures override ancestors,
    /// including empty strings and false. Rectangles replace same-named
    /// rectangles as whole values. A child text entry replaces the complete
    /// ancestor entry with the same `TextType`; missing text attributes do not
    /// acquire defaults or values from the replaced entry. Other inherited text
    /// entries remain available. Source declarations are not modified.
    ///
    /// # Errors
    /// Returns [`Error::MissingReference`] for an unknown requested name or base,
    /// or [`Error::ReferenceCycle`] when following bases revisits a style. These
    /// checks also apply after callers modify the public declaration map.
    pub fn resolve(&self, name: &str) -> Result<ResolvedStyle, Error> {
        let mut current = name;
        let mut seen = BTreeSet::new();
        let mut names = Vec::new();
        let mut chain = Vec::new();
        loop {
            // Walk by reference so long source chains require no recursive call stack.
            names.push(current.to_owned());
            if !seen.insert(current) {
                return Err(Error::ReferenceCycle(names));
            }
            let style = self
                .styles
                .get(current)
                .ok_or_else(|| Error::MissingReference {
                    kind: "style",
                    key: current.to_owned(),
                })?;
            chain.push(&style.properties);
            match style.clone_from.as_deref() {
                Some(base) => current = base,
                None => break,
            }
        }
        let mut properties = StyleProperties::default();
        for declaration in chain.into_iter().rev() {
            properties.overlay(declaration);
        }
        Ok(ResolvedStyle {
            name: name.to_owned(),
            properties,
        })
    }
}

impl StyleProperties {
    /// Apply one declaration while preserving unspecified inherited values.
    fn overlay(&mut self, child: &Self) {
        overlay_optional(&mut self.frame, &child.frame);
        overlay_optional(&mut self.watermark_effect, &child.watermark_effect);
        overlay_optional(&mut self.model, &child.model);
        overlay_optional(&mut self.art_suffix, &child.art_suffix);
        overlay_optional(
            &mut self.use_watermark_effect_on_non_foils,
            &child.use_watermark_effect_on_non_foils,
        );
        overlay_optional(&mut self.reference_size, &child.reference_size);
        self.pt_box.overlay(&child.pt_box);
        self.cc_box.overlay(&child.cc_box);
        self.rectangles.extend(
            child
                .rectangles
                .iter()
                .map(|(key, value)| (key.clone(), *value)),
        );
        self.text_elements.extend(
            child
                .text_elements
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
}

impl BoxTextures {
    /// Override only texture components explicitly supplied by a child style.
    fn overlay(&mut self, child: &Self) {
        for (target, value) in [
            (&mut self.left, &child.left),
            (&mut self.right, &child.right),
            (&mut self.middle, &child.middle),
        ] {
            overlay_optional(target, value);
        }
    }
}

/// Copy a declared optional value while leaving an absent override inherited.
fn overlay_optional<T: Clone>(target: &mut Option<T>, value: &Option<T>) {
    if value.is_some() {
        target.clone_from(value);
    }
}

/// Parse a declaration without requiring its clone base to have appeared earlier.
fn parse_style(node: Node<'_, '_>) -> Result<CardStyle, Error> {
    require_name(node, "AnimCardStyle")?;
    check_attributes(node, &["Name", "CloneFrom", "UseWatermarkEffectOnNonFoils"])?;
    let name = required_key(node, "Name")?.to_owned();
    let clone_from = node
        .attribute("CloneFrom")
        .map(|_| required_key(node, "CloneFrom").map(str::to_owned))
        .transpose()?;
    let mut properties = StyleProperties {
        use_watermark_effect_on_non_foils: node
            .attribute("UseWatermarkEffectOnNonFoils")
            .map(parse_boolean)
            .transpose()?,
        ..StyleProperties::default()
    };
    let mut seen = BTreeSet::new();
    for child in element_children(node)? {
        // Scalar and rectangle fields are singular; text entries have their own typed key.
        let field = child.tag_name().name();
        if field != "TextElementEntry" && !seen.insert(field) {
            return Err(Error::Duplicate {
                kind: "style field",
                key: field.to_owned(),
            });
        }
        match field {
            "Frame" => properties.frame = Some(scalar(child)?),
            "WatermarkEffect" => properties.watermark_effect = Some(scalar(child)?),
            "Model" => properties.model = Some(scalar(child)?),
            "ArtSuffix" => properties.art_suffix = Some(scalar(child)?),
            "PTBoxLeft" => properties.pt_box.left = Some(scalar(child)?),
            "PTBoxRight" => properties.pt_box.right = Some(scalar(child)?),
            "PTBoxMiddle" => properties.pt_box.middle = Some(scalar(child)?),
            "CCBoxLeft" => properties.cc_box.left = Some(scalar(child)?),
            "CCBoxRight" => properties.cc_box.right = Some(scalar(child)?),
            "CCBoxMiddle" => properties.cc_box.middle = Some(scalar(child)?),
            "ReferenceSize" => properties.reference_size = Some(parse_size(child)?),
            "TextElementEntry" => {
                // Duplicate text types would make whole-entry inheritance ambiguous.
                let element = parse_text(child)?;
                let key = element.text_type.clone();
                if properties
                    .text_elements
                    .insert(key.clone(), element)
                    .is_some()
                {
                    return Err(Error::Duplicate {
                        kind: "text element",
                        key,
                    });
                }
            }
            field if field.starts_with("rc") && field.len() > 2 => {
                properties
                    .rectangles
                    .insert(field.to_owned(), parse_rectangle(child)?);
            }
            _ => {
                return Err(Error::InvalidData(format!(
                    "unsupported style field: {field}"
                )));
            }
        }
    }
    Ok(CardStyle {
        name,
        clone_from,
        properties,
    })
}

/// Parse an explicit XML boolean without treating an unknown token as false.
fn parse_boolean(value: &str) -> Result<bool, Error> {
    match value {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(Error::InvalidData(format!(
            "invalid style boolean: {value:?}"
        ))),
    }
}

/// Parse the source's complete two-component logical reference size.
fn parse_size(node: Node<'_, '_>) -> Result<Size, Error> {
    let values = geometry(node, &["Width", "Height"])?;
    Ok(Size {
        width: values["Width"],
        height: values["Height"],
    })
}

/// Parse complete rectangle child elements while preserving signed origins.
fn parse_rectangle(node: Node<'_, '_>) -> Result<Rectangle, Error> {
    let values = geometry(node, &["X", "Y", "Width", "Height"])?;
    Ok(Rectangle {
        x: values["X"],
        y: values["Y"],
        width: values["Width"],
        height: values["Height"],
    })
}

/// Read complete named geometry fields and reject missing, duplicate, or unknown components.
fn geometry(node: Node<'_, '_>, required: &[&str]) -> Result<BTreeMap<String, f64>, Error> {
    check_attributes(node, &[])?;
    let mut values = BTreeMap::new();
    for child in element_children(node)? {
        // Geometry names retain their source spelling; only dimensions disallow negatives.
        let name = child.tag_name().name();
        if !required.contains(&name) {
            return Err(Error::InvalidData(format!(
                "unsupported geometry field: {name}"
            )));
        }
        let value = number(&scalar(child)?, matches!(name, "Width" | "Height"))?;
        if values.insert(name.to_owned(), value).is_some() {
            return Err(Error::Duplicate {
                kind: "geometry field",
                key: name.to_owned(),
            });
        }
    }
    for name in required {
        if !values.contains_key(*name) {
            return Err(Error::InvalidData(format!(
                "missing geometry field: {name}"
            )));
        }
    }
    Ok(values)
}

/// Parse one complete text declaration with optional whole-rectangle bounds.
fn parse_text(node: Node<'_, '_>) -> Result<TextElement, Error> {
    check_attributes(
        node,
        &["TextType", "Font", "Style", "ForeColor", "X", "Y", "W", "H"],
    )?;
    if !element_children(node)?.is_empty() {
        return Err(Error::InvalidData(
            "TextElementEntry cannot contain child elements".into(),
        ));
    }
    let bounds = ["X", "Y", "W", "H"].map(|name| node.attribute(name));
    let bounds = match bounds {
        [None, None, None, None] => None,
        [Some(x), Some(y), Some(width), Some(height)] => Some(Rectangle {
            x: number(x, false)?,
            y: number(y, false)?,
            width: number(width, true)?,
            height: number(height, true)?,
        }),
        _ => {
            return Err(Error::InvalidData(
                "text bounds require all of X, Y, W, and H".into(),
            ));
        }
    };
    Ok(TextElement {
        text_type: required_key(node, "TextType")?.to_owned(),
        font: required_attribute(node, "Font")?.to_owned(),
        fore_color: required_attribute(node, "ForeColor")?.to_owned(),
        font_style: node.attribute("Style").map(str::to_owned),
        bounds,
    })
}

/// Parse finite geometry while allowing zero dimensions and negative coordinates.
fn number(value: &str, dimension: bool) -> Result<f64, Error> {
    let parsed = value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite());
    match parsed {
        Some(number) if !dimension || number >= 0.0 => Ok(number),
        _ => Err(Error::InvalidData(format!(
            "invalid style geometry: {value:?}"
        ))),
    }
}

/// Require an unqualified element with its expected case-sensitive source name.
fn require_name(node: Node<'_, '_>, name: &str) -> Result<(), Error> {
    if node.tag_name().namespace().is_some() || node.tag_name().name() != name {
        return Err(Error::InvalidData(format!(
            "expected unqualified {name} element"
        )));
    }
    Ok(())
}

/// Reject unrecognized or namespace-qualified attributes rather than losing source values.
fn check_attributes(node: Node<'_, '_>, allowed: &[&str]) -> Result<(), Error> {
    for attribute in node.attributes() {
        if attribute.namespace().is_some() || !allowed.contains(&attribute.name()) {
            return Err(Error::InvalidData(format!(
                "unsupported {} attribute: {}",
                node.tag_name().name(),
                attribute.name()
            )));
        }
    }
    Ok(())
}

/// Collect unqualified element children while rejecting mixed nonwhitespace text.
fn element_children<'a, 'input>(node: Node<'a, 'input>) -> Result<Vec<Node<'a, 'input>>, Error> {
    let mut elements = Vec::new();
    for child in node.children() {
        // Comments and indentation are harmless; namespaces and mixed content are not declarations.
        if child.is_element() {
            if child.tag_name().namespace().is_some() {
                return Err(Error::InvalidData(
                    "namespaced style elements are unsupported".into(),
                ));
            }
            elements.push(child);
        } else if child.is_text() && child.text().is_some_and(|text| !text.trim().is_empty()) {
            return Err(Error::InvalidData(format!(
                "unexpected text inside {}",
                node.tag_name().name()
            )));
        }
    }
    Ok(elements)
}

/// Read exact decoded scalar text, allowing an explicit empty override.
fn scalar(node: Node<'_, '_>) -> Result<String, Error> {
    check_attributes(node, &[])?;
    let mut value = String::new();
    for child in node.children() {
        // Join adjacent text around comments without trimming intentional source values.
        if child.is_element() {
            return Err(Error::InvalidData(format!(
                "{} requires scalar text",
                node.tag_name().name()
            )));
        }
        if child.is_text() {
            value.push_str(child.text().unwrap_or_default());
        }
    }
    Ok(value)
}

/// Read a required plain attribute without assigning an absent value a default.
fn required_attribute<'a>(node: Node<'a, '_>, name: &str) -> Result<&'a str, Error> {
    node.attribute(name).ok_or_else(|| {
        Error::InvalidData(format!(
            "missing {} attribute: {name}",
            node.tag_name().name()
        ))
    })
}

/// Require a nonblank identifier while preserving its exact source spelling.
fn required_key<'a>(node: Node<'a, '_>, name: &str) -> Result<&'a str, Error> {
    let value = required_attribute(node, name)?;
    if value.trim().is_empty() {
        return Err(Error::InvalidData(format!(
            "{} requires a nonempty {name}",
            node.tag_name().name()
        )));
    }
    Ok(value)
}
