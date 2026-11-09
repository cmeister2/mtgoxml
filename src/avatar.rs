//! Avatar metadata and the client's regular, mini, and shader texture locations.

use crate::carddata::{
    CatalogId, Error, FriendlyObject, FriendlyValue, ResolvedObject, ResolvedValue,
};

/// Client fields needed to identify an avatar and locate its artwork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Avatar {
    /// Catalog identity used only when the artwork ID is absent or zero.
    id: CatalogId,
    /// Original artwork ID; zero retains the client's catalog fallback.
    art_id: u64,
    /// Original premium-effect discriminator, including unrecognized future values.
    premium_effect: u32,
}

impl Avatar {
    /// Read avatar fields from a fully resolved source object.
    ///
    /// Returns `None` for an object whose `IS_VANGUARD` integer flag is false or absent.
    ///
    /// # Errors
    /// Returns an invalid-value error for malformed avatar flags, artwork IDs, or effects.
    pub fn from_object(object: &ResolvedObject<'_>) -> Result<Option<Self>, Error> {
        Self::read_object(object, false)
    }

    /// Read an effect-bearing avatar, ignoring static avatars before artwork validation.
    ///
    /// # Errors
    /// Returns the errors described by [`Self::from_object`] for animated avatars.
    pub fn animated_from_object(object: &ResolvedObject<'_>) -> Result<Option<Self>, Error> {
        Self::read_object(object, true)
    }

    /// Adapt source fields once for either the general or animated-only projection.
    fn read_object(
        object: &ResolvedObject<'_>,
        animated_only: bool,
    ) -> Result<Option<Self>, Error> {
        Self::from_fields(
            object.id(),
            object.field_ref("IS_VANGUARD").map(Field::from_resolved),
            object.field_ref("ARTID").map(Field::from_resolved),
            object.field_ref("PREMIUM_EFFECT").map(Field::from_resolved),
            animated_only,
        )
    }

    /// Read the same avatar fields from an owned friendly projection.
    ///
    /// Successfully promoted artwork IDs come from `art_id`; malformed original
    /// `ARTID` values remain in `mtgo` and are rejected rather than treated as missing.
    ///
    /// # Errors
    /// Returns the same invalid-value errors as [`Self::from_object`].
    pub fn from_friendly(object: &FriendlyObject) -> Result<Option<Self>, Error> {
        Self::read_friendly(object, false)
    }

    /// Read an effect-bearing avatar from a friendly projection.
    ///
    /// Static avatars are ignored before validating their artwork metadata.
    ///
    /// # Errors
    /// Returns the errors described by [`Self::from_friendly`] for animated avatars.
    pub fn animated_from_friendly(object: &FriendlyObject) -> Result<Option<Self>, Error> {
        Self::read_friendly(object, true)
    }

    /// Adapt owned fields once for either the general or animated-only projection.
    fn read_friendly(object: &FriendlyObject, animated_only: bool) -> Result<Option<Self>, Error> {
        let art_id = object
            .art_id
            .map(Field::Number)
            .or_else(|| object.mtgo.get("ARTID").map(Field::from_friendly));
        Self::from_fields(
            object.id,
            object.mtgo.get("IS_VANGUARD").map(Field::from_friendly),
            art_id,
            object.mtgo.get("PREMIUM_EFFECT").map(Field::from_friendly),
            animated_only,
        )
    }

    /// Interpret source and friendly fields with identical client semantics.
    fn from_fields(
        id: CatalogId,
        flag: Option<Field<'_>>,
        art: Option<Field<'_>>,
        effect: Option<Field<'_>>,
        animated_only: bool,
    ) -> Result<Option<Self>, Error> {
        let enabled = integer(flag, 0).ok_or_else(|| invalid(id, "IS_VANGUARD", flag))? >= 1;
        if !enabled {
            return Ok(None);
        }
        let premium_effect = match effect {
            None => 0,
            Some(Field::Text(value)) => value
                .parse()
                .map_err(|_| invalid(id, "PREMIUM_EFFECT", effect))?,
            Some(Field::Number(value)) => {
                u32::try_from(value).map_err(|_| invalid(id, "PREMIUM_EFFECT", effect))?
            }
            _ => return Err(invalid(id, "PREMIUM_EFFECT", effect)),
        };
        if animated_only && premium_effect == 0 {
            return Ok(None);
        }
        let art_id = match art {
            None => 0,
            Some(Field::Text(value)) => {
                artwork_id(value).ok_or_else(|| invalid(id, "ARTID", art))?
            }
            Some(Field::Number(value)) if value <= i32::MAX as u64 => value,
            _ => return Err(invalid(id, "ARTID", art)),
        };
        if id == 0 || (art_id == 0 && id > i32::MAX as u64) {
            return Err(invalid(
                id,
                "DigitalObjectCatalogID",
                Some(Field::Number(id)),
            ));
        }
        Ok(Some(Self {
            id,
            art_id,
            premium_effect,
        }))
    }

    /// The avatar's catalog identity.
    pub fn id(&self) -> CatalogId {
        self.id
    }

    /// The original artwork identity, or zero for catalog-ID fallback.
    pub fn art_id(&self) -> u64 {
        self.art_id
    }

    /// The original premium-effect code; zero selects a static avatar.
    pub fn premium_effect(&self) -> u32 {
        self.premium_effect
    }

    /// The regular JPEG filename used by both avatar artwork pipelines.
    pub fn file_name(&self) -> String {
        format!("{}_avatar.jpg", file_stem(self.id, self.art_id))
    }

    /// Candidate paths used by the client's avatar renderer, relative to `artRoot`.
    ///
    /// These fixed renderer directories intentionally do not use the separately
    /// configured `avatarArtSubPath` used for primary card artwork. Known single
    /// effects have four paths, double effects have six, and unknown effect codes
    /// include both additional-texture candidates for preservation.
    pub fn asset_paths(&self) -> Vec<String> {
        let stem = file_stem(self.id, self.art_id);
        let mut paths = vec![
            format!("Avatars/Regular/{stem}_avatar.jpg"),
            format!("Avatars/Mini/{stem}_avatar.png"),
            format!("Avatars/Regular_Shader/{stem}_avatar.png"),
            format!("Avatars/Mini_Shader/{stem}_avatar.png"),
        ];
        if !matches!(self.premium_effect, 0 | 1 | 2 | 3 | 9 | 20 | 22 | 23) {
            paths.push(format!("Avatars/Regular_Shader/{stem}_add_avatar.png"));
            paths.push(format!("Avatars/Mini_Shader/{stem}_add_avatar.png"));
        }
        paths
    }
}

/// A scalar source representation shared by primary artwork and avatar metadata.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Field<'a> {
    /// An unmodified text property.
    Text(&'a str),
    /// A presence-only property.
    Flag,
    /// A numeric alias already promoted by the friendly projection.
    Number(u64),
    /// A grouped property cannot supply a scalar value.
    Group,
}

impl<'a> Field<'a> {
    /// Borrow a scalar view of a resolved source field.
    pub(crate) fn from_resolved(value: &'a ResolvedValue<'_>) -> Self {
        match value {
            ResolvedValue::Text(value) => Self::Text(value),
            ResolvedValue::Flag => Self::Flag,
            ResolvedValue::Fields(_) => Self::Group,
        }
    }

    /// Borrow a scalar view of an owned friendly field.
    fn from_friendly(value: &'a FriendlyValue) -> Self {
        match value {
            FriendlyValue::Text(value) => Self::Text(value),
            FriendlyValue::Flag => Self::Flag,
            FriendlyValue::Fields(_) => Self::Group,
        }
    }
}

/// Parse the client's signed integer convention, including presence-only flags.
pub(crate) fn integer(value: Option<Field<'_>>, default: i32) -> Option<i32> {
    match value {
        None => Some(default),
        Some(Field::Text(value)) => value.trim().parse().ok(),
        Some(Field::Flag) => Some(1),
        Some(Field::Number(value)) => i32::try_from(value).ok(),
        Some(Field::Group) => None,
    }
}

/// Parse an unsigned artwork ID within the client's signed integer bounds.
pub(crate) fn artwork_id(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok().filter(|id| *id <= i32::MAX as u64)
}

/// Format the shared artwork identity and catalog fallback marker.
pub(crate) fn file_stem(id: CatalogId, art_id: u64) -> String {
    if art_id == 0 {
        format!("{id:05}c")
    } else {
        format!("{art_id:05}")
    }
}

/// Preserve which exact property could not supply valid client metadata.
fn invalid(id: CatalogId, field: &str, value: Option<Field<'_>>) -> Error {
    Error::InvalidValue {
        object: id,
        field: field.into(),
        value: match value {
            Some(Field::Text(value)) => value.to_owned(),
            Some(Field::Flag) => "<presence flag>".into(),
            Some(Field::Number(value)) => value.to_string(),
            Some(Field::Group) => "<property group>".into(),
            None => "<missing>".into(),
        },
    }
}
