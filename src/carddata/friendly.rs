//! Owned, readable projections of resolved MTGO objects without losing other fields.

use std::collections::{BTreeMap, BTreeSet};

use super::{CatalogId, Error, ResolvedObject, ResolvedValue};

/// One of Magic's five colors, ordered white, blue, black, red, then green.
///
/// With the `serde` feature, colors serialize as `W`, `U`, `B`, `R`, and `G`.
/// Colorless objects use an empty color list rather than an additional color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Color {
    /// White, represented by `W`.
    #[cfg_attr(feature = "serde", serde(rename = "W"))]
    White,
    /// Blue, represented by `U`.
    #[cfg_attr(feature = "serde", serde(rename = "U"))]
    Blue,
    /// Black, represented by `B`.
    #[cfg_attr(feature = "serde", serde(rename = "B"))]
    Black,
    /// Red, represented by `R`.
    #[cfg_attr(feature = "serde", serde(rename = "R"))]
    Red,
    /// Green, represented by `G`.
    #[cfg_attr(feature = "serde", serde(rename = "G"))]
    Green,
}

impl Color {
    /// Recognize one exact MTGO color token without discarding unfamiliar values.
    fn from_source(value: &str) -> Option<Self> {
        match value {
            "COLOR_WHITE" => Some(Self::White),
            "COLOR_BLUE" => Some(Self::Blue),
            "COLOR_BLACK" => Some(Self::Black),
            "COLOR_RED" => Some(Self::Red),
            "COLOR_GREEN" => Some(Self::Green),
            _ => None,
        }
    }
}

/// A recognized MTGO rarity, including noncard and promotional categories.
///
/// With the `serde` feature, variants serialize as snake_case strings, such as
/// `basic_land` and `play_aid`. Unfamiliar source rarities remain in
/// [`FriendlyObject::mtgo`] rather than being assigned a known rarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Rarity {
    /// A basic land.
    BasicLand,
    /// A common object.
    Common,
    /// An uncommon object.
    Uncommon,
    /// A rare object.
    Rare,
    /// A mythic rare object.
    Mythic,
    /// A token object.
    Token,
    /// An object in a bonus rarity category.
    Bonus,
    /// An object in a promotional rarity category.
    Promo,
    /// A play aid.
    PlayAid,
}

impl Rarity {
    /// Recognize a complete source rarity token using an explicit mapping.
    fn from_source(value: &str) -> Option<Self> {
        match value {
            "MA_RARITY_BASIC_LAND" => Some(Self::BasicLand),
            "MA_RARITY_COMMON" => Some(Self::Common),
            "MA_RARITY_UNCOMMON" => Some(Self::Uncommon),
            "MA_RARITY_RARE" => Some(Self::Rare),
            "MA_RARITY_MYTHIC" => Some(Self::Mythic),
            "MA_RARITY_TOKEN" => Some(Self::Token),
            "MA_RARITY_BONUS" => Some(Self::Bonus),
            "MA_RARITY_PROMO" => Some(Self::Promo),
            "MA_RARITY_PLAY_AID" => Some(Self::PlayAid),
            _ => None,
        }
    }
}

/// An owned MTGO field that has not been consumed by a recognized friendly alias.
///
/// Strings, including empty strings and numeric-looking identifiers, stay
/// strings. With the `serde` feature, values serialize directly as a JSON
/// string, `true` for a presence flag, or an object for a nested group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FriendlyValue {
    /// Literal or resolved lookup text with MTGO formatting retained.
    Text(String),
    /// A presence-only field, serialized as `true`.
    Flag,
    /// A nested group whose source property names and value forms are preserved.
    Fields(BTreeMap<String, FriendlyValue>),
}

impl FriendlyValue {
    /// Own a resolved value while preserving its representation recursively.
    fn from_resolved(value: &ResolvedValue<'_>) -> Self {
        match value {
            ResolvedValue::Text(value) => Self::Text((*value).to_owned()),
            ResolvedValue::Flag => Self::Flag,
            ResolvedValue::Fields(fields) => Self::Fields(
                fields
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), Self::from_resolved(value)))
                    .collect(),
            ),
        }
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for FriendlyValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Text(value) => serde::Serialize::serialize(value, serializer),
            Self::Flag => serializer.serialize_bool(true),
            Self::Fields(fields) => serde::Serialize::serialize(fields, serializer),
        }
    }
}

/// A readable, owned view of a resolved card, token, product, or other object.
///
/// Only explicitly recognized fields are promoted to friendly names. A field
/// with an unfamiliar representation or value stays in [`Self::mtgo`], while
/// its optional friendly field is `None`. Missing text and explicit empty text
/// remain distinct. `serde` serialization omits optional fields that are `None`.
///
/// Printed statistics, engine identifiers, and nested groups stay in `mtgo`;
/// this projection does not infer additional aliases from source-name suffixes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct FriendlyObject {
    /// The digital-object catalog ID, including distinct foil and nonfoil objects.
    pub id: CatalogId,
    /// `CARDNAME_STRING`, retaining an explicitly empty display name.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub name: Option<String>,
    /// `ARTIST_NAME_STRING`, retaining the source's spelling and punctuation.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub artist: Option<String>,
    /// `REAL_ORACLETEXT_STRING` with MTGO formatting retained.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub rules_text: Option<String>,
    /// `FLAVORTEXT_STRING` with MTGO formatting retained.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub flavor_text: Option<String>,
    /// `MANA_COST_STRING` in MTGO's original mana notation.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub mana_cost: Option<String>,
    /// An unsigned decimal `ARTID`, independent of catalog and texture IDs.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub art_id: Option<u64>,
    /// An optional artwork URL, initially absent until a caller resolves it.
    ///
    /// [`ResolvedObject::friendly`] performs no network requests or artwork discovery.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub art_url: Option<String>,
    /// The resolved set code, falling back to the declaring fragment's ID.
    pub set: String,
    /// The declaring fragment's ID when it differs from the resolved set code.
    ///
    /// Shared metadata from [`crate::CardDatabase::set_metadata`] uses this ID
    /// when present, or [`Self::set`] otherwise. Ordinary matching set codes
    /// leave this absent.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub source_set: Option<String>,
    /// `COLLECTOR_INFO_STRING`, including letters, separators, and empty text.
    ///
    /// This is preserved as text; values such as `8/15` are not reduced to a number.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub collector_number: Option<String>,
    /// An unsigned decimal `CONVERTED_MANA_COST`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub mana_value: Option<u64>,
    /// Recognized `COLOR` tokens in WUBRG order, or an empty list for colorless.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub colors: Option<Vec<Color>>,
    /// Recognized `COLOR_IDENTITY` tokens in WUBRG order.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub color_identity: Option<Vec<Color>>,
    /// The recognized `RARITY_STATUS`, including token and product categories.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub rarity: Option<Rarity>,
    /// The effective foil property, using [`ResolvedObject::is_foil`] semantics.
    pub foil: bool,
    /// Every effective field not consumed by a recognized friendly alias.
    ///
    /// Source names, numeric-looking strings, empty text, flags, and nested
    /// namespaces are retained. Successfully promoted fields are not duplicated.
    pub mtgo: BTreeMap<String, FriendlyValue>,
}

impl ResolvedObject<'_> {
    /// Build an owned friendly view while retaining all unmapped effective fields.
    ///
    /// Text aliases are `CARDNAME_STRING`, `ARTIST_NAME_STRING`,
    /// `REAL_ORACLETEXT_STRING`, `FLAVORTEXT_STRING`, `MANA_COST_STRING`, and
    /// `COLLECTOR_INFO_STRING`. Decimal `ARTID` and `CONVERTED_MANA_COST`, known
    /// `COLOR` and `COLOR_IDENTITY` tokens, and known `RARITY_STATUS` values are
    /// normalized only when fully recognized. `CARDSETNAME_STRING` supplies the
    /// set code when it is text; otherwise the source fragment supplies it.
    ///
    /// Successfully promoted fields, including `IS_FOIL`, are removed from the
    /// retained MTGO map. Unfamiliar optional values stay there unchanged.
    /// Color lists are deduplicated into WUBRG order. Artwork URLs remain absent
    /// until resolved separately. This method performs no network requests.
    ///
    /// # Errors
    /// Returns the invalid-foil errors described by [`Self::is_foil`].
    pub fn friendly(&self) -> Result<FriendlyObject, Error> {
        let foil = self.is_foil()?;
        let mut mtgo: BTreeMap<_, _> = self
            .fields()
            .iter()
            .map(|(name, value)| ((*name).to_owned(), FriendlyValue::from_resolved(value)))
            .collect();

        // Promote only complete, recognized values; every other source field remains owned.
        let name = take_text(&mut mtgo, "CARDNAME_STRING");
        let artist = take_text(&mut mtgo, "ARTIST_NAME_STRING");
        let rules_text = take_text(&mut mtgo, "REAL_ORACLETEXT_STRING");
        let flavor_text = take_text(&mut mtgo, "FLAVORTEXT_STRING");
        let mana_cost = take_text(&mut mtgo, "MANA_COST_STRING");
        let art_id = take_parsed(&mut mtgo, "ARTID", parse_unsigned);
        let collector_number = take_text(&mut mtgo, "COLLECTOR_INFO_STRING");
        let mana_value = take_parsed(&mut mtgo, "CONVERTED_MANA_COST", parse_unsigned);
        let colors = take_parsed(&mut mtgo, "COLOR", parse_colors);
        let color_identity = take_parsed(&mut mtgo, "COLOR_IDENTITY", parse_colors);
        let rarity = take_parsed(&mut mtgo, "RARITY_STATUS", Rarity::from_source);
        let set = self.set_code().to_owned();
        let source_set = (self.set().id != set).then(|| self.set().id.clone());
        take_text(&mut mtgo, "CARDSETNAME_STRING");
        mtgo.remove("IS_FOIL");

        Ok(FriendlyObject {
            id: self.id(),
            name,
            artist,
            rules_text,
            flavor_text,
            mana_cost,
            art_id,
            art_url: None,
            set,
            source_set,
            collector_number,
            mana_value,
            colors,
            color_identity,
            rarity,
            foil,
            mtgo,
        })
    }
}

/// Move a text alias out of the map, leaving other representations unchanged.
fn take_text(fields: &mut BTreeMap<String, FriendlyValue>, name: &str) -> Option<String> {
    match fields.remove(name) {
        Some(FriendlyValue::Text(value)) => Some(value),
        Some(value) => {
            fields.insert(name.to_owned(), value);
            None
        }
        None => None,
    }
}

/// Remove a text field only after the complete value has been recognized.
fn take_parsed<T>(
    fields: &mut BTreeMap<String, FriendlyValue>,
    name: &str,
    parse: impl FnOnce(&str) -> Option<T>,
) -> Option<T> {
    let parsed = match fields.get(name) {
        Some(FriendlyValue::Text(value)) => parse(value),
        Some(FriendlyValue::Flag | FriendlyValue::Fields(_)) | None => None,
    };
    if parsed.is_some() {
        fields.remove(name);
    }
    parsed
}

/// Recognize unsigned ASCII decimal text without trimming or accepting other syntax.
fn parse_unsigned(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// Recognize a complete color token list, rejecting mixed colorless or unknown tokens.
fn parse_colors(value: &str) -> Option<Vec<Color>> {
    if value == "COLOR_COLORLESS" {
        return Some(Vec::new());
    }
    let colors: Option<BTreeSet<_>> = value.split('|').map(Color::from_source).collect();
    Some(colors?.into_iter().collect())
}
