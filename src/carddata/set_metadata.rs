//! Shared source metadata collected once for each set code.

use std::collections::{BTreeMap, btree_map::Entry};

use super::{CardDatabase, Error};

/// Metadata shared by source fragments declaring the same set code.
///
/// These values retain MTGO's spelling and optionality. The source's `age` is
/// not interpreted as a date, and an absent `kind` differs from an empty one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SetMetadata {
    /// The source set's optional, uninterpreted `age` value.
    pub age: Option<u32>,
    /// The source set's optional `cardsetType`, preserving its exact spelling.
    pub kind: Option<String>,
}

impl CardDatabase {
    /// Collect shared metadata once per set code, sorted by code.
    ///
    /// All loaded source fragments participate, including fragments with no
    /// digital objects. Matching fragments contribute a single entry. This
    /// leaves the database and its original fragment metadata unchanged.
    ///
    /// # Errors
    /// Returns [`Error::InvalidData`] when fragments declaring the same set code
    /// disagree about `age` or `kind`. Missing and explicitly empty values remain
    /// distinct, and `kind` values are compared without normalization.
    pub fn set_metadata(&self) -> Result<BTreeMap<String, SetMetadata>, Error> {
        let mut metadata = BTreeMap::new();
        for set in self.sets() {
            let incoming = SetMetadata {
                age: set.age,
                kind: set.kind.clone(),
            };
            match metadata.entry(set.id.clone()) {
                Entry::Vacant(entry) => {
                    entry.insert(incoming);
                }
                Entry::Occupied(entry) if entry.get() != &incoming => {
                    return Err(Error::InvalidData(format!(
                        "conflicting age or kind metadata for set {}",
                        set.id
                    )));
                }
                Entry::Occupied(_) => {}
            }
        }
        Ok(metadata)
    }
}
