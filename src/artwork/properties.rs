//! Read effective card properties using the client's artwork-selection semantics.

use super::color::ColorFlags;

use crate::{ResolvedObject, carddata::ResolvedValue};

/// Read the client's integer boolean, including presence-only XML flags.
pub(super) fn boolean(object: &ResolvedObject<'_>, name: &str) -> Option<bool> {
    integer(object, name, 0).map(|value| value >= 1)
}

/// Read a signed client integer without treating malformed text as a default.
pub(super) fn integer(object: &ResolvedObject<'_>, name: &str, default: i32) -> Option<i32> {
    match object.field_ref(name) {
        None => Some(default),
        Some(ResolvedValue::Flag) => Some(1),
        Some(ResolvedValue::Text(value)) => value.trim().parse().ok(),
        Some(ResolvedValue::Fields(_)) => None,
    }
}

/// Whether an effective string is empty, with the client's missing-string default.
pub(super) fn text_empty(object: &ResolvedObject<'_>, name: &str) -> Option<bool> {
    match object.field_ref(name) {
        None => Some(true),
        Some(ResolvedValue::Text(value)) => Some(value.is_empty()),
        Some(ResolvedValue::Flag) => Some(false),
        Some(ResolvedValue::Fields(_)) => None,
    }
}

/// Whether the client's effective rules text is empty, including dungeon rooms.
pub(super) fn rules_empty(object: &ResolvedObject<'_>) -> Option<bool> {
    if !boolean(object, "IS_DUNGEON")? {
        return text_empty(object, "REAL_ORACLETEXT_STRING");
    }
    if !trimmed_empty(object.field_ref("REAL_ORACLETEXT_STRING"))? {
        return Some(false);
    }
    for (name, value) in object.fields() {
        // Dungeon rules append the rules text from each tracker-state sublist.
        if name.starts_with("TRACKER_STATE_LIST") {
            let ResolvedValue::Fields(fields) = value else {
                return None;
            };
            if !trimmed_empty(fields.get("TRACKER_STATE_RULES_TEXT_STRING"))? {
                return Some(false);
            }
        }
    }
    Some(true)
}

/// Whether a string contributes no nonwhitespace text to assembled dungeon rules.
fn trimmed_empty(value: Option<&ResolvedValue<'_>>) -> Option<bool> {
    match value {
        None => Some(true),
        Some(ResolvedValue::Text(value)) => Some(value.trim().is_empty()),
        Some(ResolvedValue::Flag) => Some(false),
        Some(ResolvedValue::Fields(_)) => None,
    }
}

/// Parse MTGO named or server-bitmask colors into the client's color flags.
pub(super) fn colors(object: &ResolvedObject<'_>) -> Option<ColorFlags> {
    let spec = match object.field_ref("COLOR") {
        None => return Some(ColorFlags::COLORLESS),
        Some(ResolvedValue::Text(value)) => *value,
        Some(ResolvedValue::Flag | ResolvedValue::Fields(_)) => return None,
    };
    ColorFlags::from_spec(spec)
}
