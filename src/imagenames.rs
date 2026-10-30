#![doc = include_str!("../docs/image-names.md")]

use std::io::Read;

use regex::{Captures, Regex, RegexBuilder};
use roxmltree::Node;

use crate::{ResolvedObject, carddata::CatalogId};

pub use crate::dataxml::{Error, ParseOptions};

/// Maximum compiled size of one source regular expression.
const MAX_REGEX_BYTES: usize = 1024 * 1024;

/// The client's matching operation for one digital-object image rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum MatchRule {
    /// Match the complete trimmed name, with case-sensitive comparison.
    Equals,
    /// Match a case-sensitive substring of the trimmed name.
    Contains,
    /// Match a case-sensitive suffix of the trimmed name.
    EndsWith,
    /// Apply the source regular expression without case sensitivity.
    Regex,
    /// Match the decimal catalog ID against the literal source pattern.
    CatalogId,
}

/// One case-sensitive substitution of a set code before filename generation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct CardSetNameReplacement {
    /// Exact source set code to replace.
    pub original: String,
    /// Replacement set code, preserving its original spelling.
    pub replacement: String,
}

/// One ordered source rule and its validated filename template.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ImageNameMatch {
    /// Source matching operation.
    rule: MatchRule,
    /// Exact matching pattern, without lowercasing or other normalization.
    match_pattern: String,
    /// Source filename template, including numeric captures and placeholders.
    value_format: String,
    /// Whether `{1}` comes from the text before the matching pattern.
    is_redemption: bool,
    /// Compiled case-insensitive regex, absent for ordinary string rules.
    #[cfg_attr(feature = "serde", serde(skip))]
    regex: Option<Regex>,
}

impl ImageNameMatch {
    /// The source matching operation.
    pub fn rule(&self) -> MatchRule {
        self.rule
    }

    /// The original matching pattern.
    pub fn match_pattern(&self) -> &str {
        &self.match_pattern
    }

    /// The original filename format, including its source placeholders.
    pub fn value_format(&self) -> &str {
        &self.value_format
    }

    /// Whether deck-name formatting uses the prefix for a redemption object.
    pub fn is_redemption(&self) -> bool {
        self.is_redemption
    }

    /// Evaluate one rule without skipping a successful empty result.
    fn evaluate(&self, name: &str, set: &str, id: i32) -> Result<Option<String>, Error> {
        let matched = match self.rule {
            MatchRule::Equals => name == self.match_pattern,
            MatchRule::Contains => name.contains(&self.match_pattern),
            MatchRule::EndsWith => name.ends_with(&self.match_pattern),
            MatchRule::Regex => self
                .regex
                .as_ref()
                .is_some_and(|regex| regex.is_match(name)),
            MatchRule::CatalogId => id.to_string() == self.match_pattern,
        };
        if !matched {
            return Ok(None);
        }
        let value = if self.value_format.contains("{1}") {
            // The client handles deck/redemption placeholders before regex replacements.
            let deck = self.deck_name(name)?;
            composite_format(&self.value_format, set, &deck)?
        } else if let Some(regex) = &self.regex {
            // Regex matching uses the original name; replacement uses its lowercase form.
            let lower = name.to_lowercase();
            let replaced = regex.replace_all(&lower, |captures: &Captures<'_>| {
                regex_replacement(captures, &self.value_format)
            });
            if replaced.trim().is_empty() {
                composite_format(&self.value_format, set, "")?
            } else {
                // Preserve literal template case while normalizing only the generated text.
                let value = replaced.replace(' ', "_").replace([':', '\''], "");
                if value.contains("{0}") {
                    composite_format(&value, set, "")?
                } else {
                    value
                }
            }
        } else {
            composite_format(&self.value_format, set, "")?
        };
        Ok(Some(value.replace(',', "")))
    }

    /// Normalize the client's deck-name portion for its `{1}` placeholder.
    fn deck_name(&self, name: &str) -> Result<String, Error> {
        let at = if self.rule == MatchRule::EndsWith {
            name.rfind(&self.match_pattern)
        } else {
            name.find(&self.match_pattern)
        }
        .ok_or_else(|| invalid("deck placeholder requires a literal matching pattern"))?;
        let before = name[..at].trim();
        let part = if self.is_redemption {
            before
        } else {
            // Preserve the native prefix-trimming calculation rather than guessing a deck name.
            name.get(before.len() + self.match_pattern.len()..)
                .ok_or_else(|| invalid("deck placeholder exceeds the matching name"))?
        };
        let mut result = String::new();
        for character in part.trim().to_lowercase().chars() {
            // These substitutions are the client's deck/redemption filename convention.
            match character {
                ' ' | '-' => result.push('_'),
                '&' => result.push_str("and"),
                '\'' | '(' | ')' | ':' | '\\' => {}
                other => result.push(other),
            }
        }
        Ok(result)
    }
}

/// Parsed, ordered image-name rules independent of networking and image loading.
///
/// Both rule and set-replacement order are significant: the first match wins.
/// Exact duplicate entries are retained because the client processes arrays,
/// not a dictionary keyed by matching pattern or set code.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct DigitalObjectImageNames {
    /// Set substitutions in their original source order.
    replacements: Vec<CardSetNameReplacement>,
    /// Image matching rules in their original source order.
    matches: Vec<ImageNameMatch>,
}

impl DigitalObjectImageNames {
    /// Parse a Unicode document with the default byte and node limits.
    ///
    /// # Errors
    /// Returns an error for malformed or unsupported XML, exceeded limits,
    /// missing attributes, unknown rule kinds, or unsupported regex/templates.
    pub fn parse(xml: &str) -> Result<Self, Error> {
        Self::parse_with_options(xml, ParseOptions::default())
    }

    /// Parse a Unicode document using caller-selected limits.
    ///
    /// # Errors
    /// Returns the structural, regex, template, and limit errors described by [`Self::parse`].
    pub fn parse_with_options(xml: &str, options: ParseOptions) -> Result<Self, Error> {
        let document = options.document(xml)?;
        let root = document.root_element();
        if root.tag_name().name() != "DigitalObjectImageNamesData"
            || root.tag_name().namespace().is_some()
        {
            return Err(invalid(
                "expected an unqualified DigitalObjectImageNamesData root",
            ));
        }
        require_attributes(root, &[])?;
        let mut replacements = Vec::new();
        let mut matches = Vec::new();
        for node in root.children() {
            // Comments and whitespace are harmless; other content must obey the source schema.
            if node.is_comment()
                || node.is_pi()
                || node.is_text() && node.text().is_some_and(|text| text.trim().is_empty())
            {
                continue;
            }
            if !node.is_element() || node.tag_name().namespace().is_some() {
                return Err(invalid("unexpected content in DigitalObjectImageNamesData"));
            }
            require_empty(node)?;
            match node.tag_name().name() {
                "CardSetNameReplacement" => {
                    // Keep duplicates and first-match ordering exactly as declared by the client.
                    require_attributes(node, &["Original", "Replacement"])?;
                    replacements.push(CardSetNameReplacement {
                        original: required_attribute(node, "Original")?.to_owned(),
                        replacement: required_attribute(node, "Replacement")?.to_owned(),
                    });
                }
                "Match" => matches.push(parse_match(node)?),
                _ => return Err(invalid("unexpected digital-object image-name element")),
            }
        }
        Ok(Self {
            replacements,
            matches,
        })
    }

    /// Decode and parse an XML byte document with default limits.
    ///
    /// # Errors
    /// Returns decoding errors and the parsing errors described by [`Self::parse`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_bytes_with_options(bytes, ParseOptions::default())
    }

    /// Decode and parse an XML byte document using caller-selected limits.
    ///
    /// # Errors
    /// Returns decoding errors and the parsing errors described by [`Self::parse`].
    pub fn from_bytes_with_options(bytes: &[u8], options: ParseOptions) -> Result<Self, Error> {
        let xml = options.decode(bytes)?;
        Self::parse_with_options(&xml, options)
    }

    /// Read and parse one bounded XML document.
    ///
    /// # Errors
    /// Returns reader, decoding, structural, regex, template, and limit errors.
    pub fn from_reader(reader: impl Read) -> Result<Self, Error> {
        Self::from_reader_with_options(reader, ParseOptions::default())
    }

    /// Read and parse one XML document using caller-selected limits.
    ///
    /// # Errors
    /// Returns reader, decoding, structural, regex, template, and limit errors.
    pub fn from_reader_with_options(
        reader: impl Read,
        options: ParseOptions,
    ) -> Result<Self, Error> {
        let bytes = options.read(reader)?;
        Self::from_bytes_with_options(&bytes, options)
    }

    /// Set substitutions in source order, including repeated original codes.
    pub fn replacements(&self) -> &[CardSetNameReplacement] {
        &self.replacements
    }

    /// Image matching rules in source order.
    pub fn matches(&self) -> &[ImageNameMatch] {
        &self.matches
    }

    /// Apply the first exact, case-sensitive set-code replacement.
    pub fn replace_set_name<'a>(&'a self, set: &'a str) -> &'a str {
        self.replacements
            .iter()
            .find(|item| item.original == set)
            .map_or(set, |item| item.replacement.as_str())
    }

    /// Generate the client's store-product filename from a name, set code, and catalog ID.
    ///
    /// The name is trimmed, set replacements are applied once, and the first
    /// matching rule wins. Returns `Ok(None)` when no rule matches and
    /// `Ok(Some(String::new()))` when the winning rule intentionally returns an
    /// empty filename. The latter must not continue to a later catch-all rule.
    /// Filenames retain the client's template case and punctuation substitutions.
    ///
    /// Regex rules use the Rust regex engine's Unicode case-insensitive matching,
    /// not arbitrary .NET regex features. Lookarounds, backreferences, and
    /// replacement syntax other than `$N` and `$$` fail during parsing. Deck-name
    /// lowercasing is independent of the machine's current culture.
    ///
    /// The result is an unvalidated filename, not a URL or a local filesystem path.
    /// The caller chooses the relevant image directory and availability fallback.
    ///
    /// # Errors
    /// Returns an error when the catalog ID exceeds the client's signed 32-bit
    /// range, a deck placeholder cannot locate its literal pattern, or generated
    /// text contains an invalid composite format.
    pub fn generate_image_name(
        &self,
        name: &str,
        set: &str,
        catalog_id: CatalogId,
    ) -> Result<Option<String>, Error> {
        let id = i32::try_from(catalog_id)
            .map_err(|_| invalid("catalog ID exceeds the client integer range"))?;
        let name = name.trim();
        let set = self.replace_set_name(set);
        for rule in &self.matches {
            if let Some(value) = rule.evaluate(name, set, id)? {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    /// Generate a filename using an object's resolved name, declaring set, and catalog ID.
    ///
    /// This does not choose the object's image pipeline. Clone inheritance is
    /// supplied by [`ResolvedObject`]. The client uses the enclosing `CardSet`
    /// code, independently of `CARDSETNAME_STRING`; cross-set clones use their
    /// own declaring set. A missing name defaults to an empty string, so a
    /// catalog-ID rule can still match.
    ///
    /// # Errors
    /// Returns the resolution errors described by [`Self::generate_image_name`].
    pub fn for_object(&self, object: &ResolvedObject<'_>) -> Result<Option<String>, Error> {
        self.generate_image_name(
            object.name().unwrap_or_default(),
            &object.set().id,
            object.id(),
        )
    }
}

/// Parse and compile one source match without changing its source text.
fn parse_match(node: Node<'_, '_>) -> Result<ImageNameMatch, Error> {
    require_attributes(
        node,
        &["Rule", "MatchPattern", "ValueFormat", "IsRedemption"],
    )?;
    let rule = match required_attribute(node, "Rule")? {
        "Equals" => MatchRule::Equals,
        "Contains" => MatchRule::Contains,
        "EndsWith" => MatchRule::EndsWith,
        "Regex" => MatchRule::Regex,
        "CatalogId" => MatchRule::CatalogId,
        _ => return Err(invalid("unsupported image-name matching rule")),
    };
    let match_pattern = required_attribute(node, "MatchPattern")?.to_owned();
    let value_format = required_attribute(node, "ValueFormat")?.to_owned();
    let is_redemption = match node.attribute("IsRedemption").map(str::trim) {
        None | Some("false" | "0") => false,
        Some("true" | "1") => true,
        _ => return Err(invalid("IsRedemption must be an XML boolean")),
    };
    let regex = if rule == MatchRule::Regex {
        // Compile once with explicit memory and nesting bounds, before evaluating any names.
        let regex = RegexBuilder::new(&match_pattern)
            .case_insensitive(true)
            .size_limit(MAX_REGEX_BYTES)
            .nest_limit(64)
            .build()
            .map_err(|error| invalid(&format!("unsupported image-name regex: {error}")))?;
        validate_replacement(&value_format, regex.captures_len())?;
        Some(regex)
    } else {
        None
    };
    composite_format(&value_format, "", "")?;
    Ok(ImageNameMatch {
        rule,
        match_pattern,
        value_format,
        is_redemption,
        regex,
    })
}

/// Restrict an element's attributes to the unqualified fields understood by this parser.
fn require_attributes(node: Node<'_, '_>, allowed: &[&str]) -> Result<(), Error> {
    if node
        .attributes()
        .any(|attribute| attribute.namespace().is_some() || !allowed.contains(&attribute.name()))
    {
        return Err(invalid(
            "unsupported attribute in digital-object image-name data",
        ));
    }
    Ok(())
}

/// Read a required plain attribute while preserving explicitly empty values.
fn required_attribute<'a>(node: Node<'a, '_>, name: &str) -> Result<&'a str, Error> {
    node.attribute(name)
        .ok_or_else(|| invalid(&format!("missing {name} attribute")))
}

/// Require source records to be empty elements apart from whitespace and comments.
fn require_empty(node: Node<'_, '_>) -> Result<(), Error> {
    if node.children().any(|child| {
        child.is_element()
            || child.is_text() && child.text().is_some_and(|text| !text.trim().is_empty())
    }) {
        return Err(invalid("image-name rules cannot contain child data"));
    }
    Ok(())
}

/// Validate numeric regex replacements without silently adopting Rust's token syntax.
fn validate_replacement(value: &str, capture_count: usize) -> Result<(), Error> {
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        // .NET's supported numeric replacement tokens are distinct from composite {0}/{1} fields.
        if character != '$' {
            continue;
        }
        if characters.next_if_eq(&'$').is_some() {
            continue;
        }
        let mut digits = String::new();
        while characters.peek().is_some_and(char::is_ascii_digit) {
            if let Some(digit) = characters.next() {
                digits.push(digit);
            }
        }
        let index = digits
            .parse::<usize>()
            .map_err(|_| invalid("unsupported .NET regex replacement token"))?;
        if index >= capture_count {
            return Err(invalid(
                "regex replacement refers to an absent capture group",
            ));
        }
    }
    Ok(())
}

/// Expand validated .NET-style numeric captures, preserving literal template case.
fn regex_replacement(captures: &Captures<'_>, value: &str) -> String {
    let mut result = String::new();
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        // Validation ensures each dollar sign begins either a numeric capture or an escaped dollar.
        if character != '$' {
            result.push(character);
            continue;
        }
        if characters.next_if_eq(&'$').is_some() {
            result.push('$');
            continue;
        }
        let mut index = 0;
        while characters.peek().is_some_and(char::is_ascii_digit) {
            if let Some(digit) = characters.next() {
                index = index * 10 + (digit as usize - '0' as usize);
            }
        }
        if let Some(capture) = captures.get(index) {
            result.push_str(capture.as_str());
        }
    }
    result
}

/// Expand only the client's set/deck placeholders and escaped literal braces.
fn composite_format(value: &str, set: &str, deck: &str) -> Result<String, Error> {
    let mut result = String::new();
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        // Replacement values are appended literally and never recursively interpreted as templates.
        match character {
            '{' if characters.next_if_eq(&'{').is_some() => result.push('{'),
            '}' if characters.next_if_eq(&'}').is_some() => result.push('}'),
            '{' => {
                // The current native contract supplies exactly the set and deck arguments.
                let field = characters.next();
                if characters.next() != Some('}') {
                    return Err(invalid("invalid image-name format placeholder"));
                }
                match field {
                    Some('0') => result.push_str(set),
                    Some('1') => result.push_str(deck),
                    _ => return Err(invalid("unsupported image-name format placeholder")),
                }
            }
            '}' => return Err(invalid("unmatched closing brace in image-name format")),
            other => result.push(other),
        }
    }
    Ok(result)
}

/// Describe invalid or unsupported naming data without claiming successful native resolution.
fn invalid(message: &str) -> Error {
    Error::InvalidData(message.to_owned())
}
