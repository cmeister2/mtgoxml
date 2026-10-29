//! Version-bound object storage, lookup resolution, and clone inheritance.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;

use clickonce::{AssemblyIdentity, AssemblyVersion};

use crate::{Application, Url};

use super::{CardDataFile, CardSet, CatalogId, DigitalObject, Error, FieldValue, LookupTable};

/// Maximum nested property-group depth accepted during reference resolution.
const MAX_GROUP_DEPTH: usize = 64;

/// Parsed CardDataSource records belonging to one application-manifest snapshot.
///
/// Files may be inserted in any order. Resolution requires all lookup tables and
/// clone bases needed by the requested object. Multiple set fragments with the
/// same set ID are retained, including the separate noncard `_DO.xml` files.
#[derive(Debug, Clone)]
pub struct CardDatabase {
    /// The application-manifest URL identifying this deployment snapshot.
    application_url: Url,
    /// The identity required for every inserted file's deployment context.
    identity: AssemblyIdentity,
    /// Original set fragments, retaining their source records and metadata.
    sets: Vec<CardSet>,
    /// Object IDs mapped to their set-fragment and object positions.
    objects: BTreeMap<CatalogId, (usize, usize)>,
    /// Lookup tables keyed by their XML table names.
    lookups: BTreeMap<String, LookupTable>,
}

impl CardDatabase {
    /// Create an empty database bound to the supplied application snapshot.
    pub fn new(application: &Application) -> Self {
        Self {
            application_url: application.url().clone(),
            identity: application.manifest().identity.clone(),
            sets: Vec::new(),
            objects: BTreeMap::new(),
            lookups: BTreeMap::new(),
        }
    }

    /// The application-manifest URL to which this database is bound.
    pub fn application_url(&self) -> &Url {
        &self.application_url
    }

    /// The application identity associated with all inserted records.
    pub fn identity(&self) -> &AssemblyIdentity {
        &self.identity
    }

    /// The application's declared four-part assembly version.
    pub fn version(&self) -> Option<AssemblyVersion> {
        self.identity.version
    }

    /// Insert a parsed component from the supplied application snapshot.
    ///
    /// The application argument records the caller's source context; it cannot
    /// prove where externally supplied bytes originated. Lookup-table names and
    /// object IDs must be unique across the database. A failed insertion leaves
    /// all existing records unchanged.
    ///
    /// # Errors
    /// Returns [`Error::SourceMismatch`] for another application URL or identity,
    /// [`Error::Duplicate`] for an existing table or object ID, and
    /// [`Error::InvalidData`] for an empty set or lookup-table name.
    pub fn insert(&mut self, application: &Application, file: CardDataFile) -> Result<(), Error> {
        if application.url() != &self.application_url
            || application.manifest().identity != self.identity
        {
            return Err(Error::SourceMismatch);
        }
        match file {
            CardDataFile::Set(set) => self.insert_set(set),
            CardDataFile::Lookup(table) => self.insert_lookup(table),
        }
    }

    /// Retrieve an original object without resolving lookups or clone ancestry.
    pub fn raw_object(&self, id: CatalogId) -> Option<&DigitalObject> {
        self.raw_entry(id).map(|(_, object)| object)
    }

    /// Iterate original set fragments in insertion order, including `_DO` files.
    pub fn sets(&self) -> impl ExactSizeIterator<Item = &CardSet> {
        self.sets.iter()
    }

    /// Iterate original lookup tables in table-name order.
    pub fn lookups(&self) -> impl ExactSizeIterator<Item = &LookupTable> {
        self.lookups.values()
    }

    /// Iterate all loaded object IDs in ascending numeric order.
    pub fn object_ids(&self) -> impl ExactSizeIterator<Item = CatalogId> + '_ {
        self.objects.keys().copied()
    }

    /// The number of loaded digital objects, including noncard objects.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether no digital objects have been loaded, regardless of lookup tables.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Resolve an object by applying clone overrides and joining lookup IDs.
    ///
    /// Child fields replace inherited fields, including empty literal values,
    /// presence flags, and complete nested groups. Nested groups are replaced as
    /// whole properties rather than merged recursively. Only effective fields
    /// require lookup entries. Clone resolution is iterative, so long ancestry
    /// chains do not consume the call stack.
    ///
    /// # Errors
    /// Returns [`Error::MissingObject`] for an unloaded object or clone base,
    /// [`Error::CloneCycle`] for cyclic ancestry, [`Error::InvalidValue`] for an
    /// invalid clone ID, [`Error::MissingLookup`] for an unresolved reference, or
    /// [`Error::InvalidData`] for property groups nested more than 64 levels.
    pub fn object(&self, id: CatalogId) -> Result<ResolvedObject<'_>, Error> {
        let (set, raw) = self.raw_entry(id).ok_or(Error::MissingObject(id))?;
        let ancestry = self.ancestry(id)?;
        let mut inherited = BTreeMap::new();
        for object in ancestry.into_iter().rev() {
            for (name, value) in &object.fields {
                inherited.insert(name.as_str(), value);
            }
        }
        let mut fields = BTreeMap::new();
        for (name, value) in inherited {
            fields.insert(name, self.resolve_field(name, value)?);
        }
        Ok(ResolvedObject { raw, set, fields })
    }

    /// Resolve a card while requiring its effective object type to be `CARD`.
    ///
    /// # Errors
    /// Returns the errors described by [`Self::object`], or [`Error::NotCard`]
    /// when the resolved object is a booster, container, or another object type.
    pub fn card(&self, id: CatalogId) -> Result<CardView<'_>, Error> {
        let object = self.object(id)?;
        if object.object_type() != Some("CARD") {
            return Err(Error::NotCard(id));
        }
        Ok(CardView { object })
    }

    /// Resolve all currently loaded objects in numeric ID order.
    ///
    /// Each iterator item may contain the errors described by [`Self::object`].
    pub fn objects(&self) -> impl Iterator<Item = Result<ResolvedObject<'_>, Error>> + '_ {
        self.object_ids().map(|id| self.object(id))
    }

    /// Check all loaded object references, clone ancestry, and foil flags.
    ///
    /// Partial databases may be built in any order; validation is explicit so
    /// missing files can be added before requiring complete resolution.
    ///
    /// # Errors
    /// Returns the first resolution error in ascending object-ID order, including
    /// missing lookup entries or clone bases, cycles, and invalid foil flags.
    pub fn validate(&self) -> Result<(), Error> {
        for object in self.objects() {
            object?.is_foil()?;
        }
        Ok(())
    }

    /// Retrieve an original object's declaring fragment and record together.
    fn raw_entry(&self, id: CatalogId) -> Option<(&CardSet, &DigitalObject)> {
        let (set_position, object_position) = self.objects.get(&id)?;
        let set = self.sets.get(*set_position)?;
        let object = set.objects.get(*object_position)?;
        Some((set, object))
    }

    /// Check every incoming object ID before changing the object index.
    fn insert_set(&mut self, set: CardSet) -> Result<(), Error> {
        if set.id.is_empty() {
            return Err(Error::InvalidData("set ID must not be empty".to_owned()));
        }
        let mut ids = BTreeSet::new();
        for object in &set.objects {
            if !ids.insert(object.id) || self.objects.contains_key(&object.id) {
                return Err(Error::Duplicate {
                    kind: "object ID",
                    key: object.id.to_string(),
                });
            }
        }
        let set_position = self.sets.len();
        for (object_position, object) in set.objects.iter().enumerate() {
            self.objects
                .insert(object.id, (set_position, object_position));
        }
        self.sets.push(set);
        Ok(())
    }

    /// Reject duplicate table names before replacing any loaded lookup data.
    fn insert_lookup(&mut self, table: LookupTable) -> Result<(), Error> {
        if table.name.is_empty() {
            return Err(Error::InvalidData(
                "lookup-table name must not be empty".to_owned(),
            ));
        }
        if self.lookups.contains_key(&table.name) {
            return Err(Error::Duplicate {
                kind: "lookup table",
                key: table.name,
            });
        }
        self.lookups.insert(table.name.clone(), table);
        Ok(())
    }

    /// Collect descendant-to-ancestor records while detecting missing or cyclic bases.
    fn ancestry(&self, id: CatalogId) -> Result<Vec<&DigitalObject>, Error> {
        let mut ancestry = Vec::new();
        let mut visited = BTreeSet::new();
        let mut current = id;
        loop {
            // Follow one clone edge at a time and retain the traversed IDs so
            // cycles can be reported without recursive calls or stack growth.
            if !visited.insert(current) {
                // Include the repeated target so callers can identify the edge
                // that closes the cycle from the reported traversal.
                let mut cycle: Vec<_> = ancestry
                    .iter()
                    .map(|object: &&DigitalObject| object.id)
                    .collect();
                cycle.push(current);
                return Err(Error::CloneCycle(cycle));
            }
            let object = self
                .raw_object(current)
                .ok_or(Error::MissingObject(current))?;
            ancestry.push(object);
            let Some(parent) = clone_id(object)? else {
                return Ok(ancestry);
            };
            current = parent;
        }
    }

    /// Join a reference through its own field's table without ordinal assumptions.
    fn resolve_field<'a>(
        &'a self,
        name: &'a str,
        value: &'a FieldValue,
    ) -> Result<ResolvedValue<'a>, Error> {
        self.resolve_field_at_depth(name, value, 0)
    }

    /// Resolve nested groups with an explicit bound for manually built raw data.
    fn resolve_field_at_depth<'a>(
        &'a self,
        name: &'a str,
        value: &'a FieldValue,
        depth: usize,
    ) -> Result<ResolvedValue<'a>, Error> {
        match value {
            FieldValue::Literal(value) => Ok(ResolvedValue::Text(value)),
            FieldValue::Flag => Ok(ResolvedValue::Flag),
            FieldValue::Fields(fields) => {
                // Bound recursive group resolution before borrowing its children;
                // each nested reference still uses its own property table name.
                if depth >= MAX_GROUP_DEPTH {
                    return Err(Error::InvalidData(format!(
                        "property groups exceed {MAX_GROUP_DEPTH} nested levels"
                    )));
                }
                let mut resolved = BTreeMap::new();
                for (name, value) in fields {
                    resolved.insert(
                        name.as_str(),
                        self.resolve_field_at_depth(name, value, depth + 1)?,
                    );
                }
                Ok(ResolvedValue::Fields(resolved))
            }
            FieldValue::Reference(id) => self
                .lookups
                .get(name)
                .and_then(|table| table.entries.get(id))
                .map(|value| ResolvedValue::Text(value))
                .ok_or_else(|| Error::MissingLookup {
                    table: name.to_owned(),
                    id: id.clone(),
                }),
        }
    }
}

/// A resolved field, borrowing its exact text from a source record or lookup table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedValue<'a> {
    /// Literal or lookup text, retaining MTGO formatting and empty strings.
    Text(&'a str),
    /// A presence-only field without an `id` or `value` attribute.
    Flag,
    /// A nested property group with every child lookup resolved independently.
    Fields(BTreeMap<&'a str, ResolvedValue<'a>>),
}

/// A digital object with clone inheritance and lookup references resolved.
#[derive(Debug, Clone)]
pub struct ResolvedObject<'a> {
    /// The original child record, preserving its own fields and overrides.
    raw: &'a DigitalObject,
    /// The fragment in which the child record is declared.
    set: &'a CardSet,
    /// Effective inherited and overridden fields after lookup resolution.
    fields: BTreeMap<&'a str, ResolvedValue<'a>>,
}

impl<'a> ResolvedObject<'a> {
    /// The numeric digital-object catalog ID of the requested child record.
    pub fn id(&self) -> CatalogId {
        self.raw.id
    }

    /// The original child record before clone inheritance or lookup resolution.
    pub fn raw(&self) -> &'a DigitalObject {
        self.raw
    }

    /// The set fragment that declares the requested child record.
    pub fn set(&self) -> &'a CardSet {
        self.set
    }

    /// All effective fields, including unrecognized source property names.
    pub fn fields(&self) -> &BTreeMap<&'a str, ResolvedValue<'a>> {
        &self.fields
    }

    /// Retrieve an effective field by its exact XML property name.
    ///
    /// Nested groups are cloned; use [`Self::field_ref`] to borrow them instead.
    pub fn field(&self, name: &str) -> Option<ResolvedValue<'a>> {
        self.fields.get(name).cloned()
    }

    /// Borrow an effective field, including a nested group, without cloning it.
    pub fn field_ref(&self, name: &str) -> Option<&ResolvedValue<'a>> {
        self.fields.get(name)
    }

    /// Retrieve an effective text field, preserving an explicitly empty string.
    ///
    /// Absent, presence-only, and grouped fields return `None`;
    /// [`Self::field_ref`] preserves the difference between those cases.
    pub fn text(&self, name: &str) -> Option<&'a str> {
        match self.field_ref(name)? {
            ResolvedValue::Text(value) => Some(*value),
            ResolvedValue::Flag | ResolvedValue::Fields(_) => None,
        }
    }

    /// The resolved display name, when present.
    pub fn name(&self) -> Option<&'a str> {
        self.text("CARDNAME_STRING")
    }

    /// Rules text with the original MTGO formatting preserved.
    pub fn rules_text(&self) -> Option<&'a str> {
        self.text("REAL_ORACLETEXT_STRING")
    }

    /// The resolved set code, falling back to the declaring fragment's ID.
    pub fn set_code(&self) -> &'a str {
        self.text("CARDSETNAME_STRING").unwrap_or(&self.set.id)
    }

    /// Mana cost in MTGO's original text representation.
    pub fn mana_cost(&self) -> Option<&'a str> {
        self.text("MANA_COST_STRING")
    }

    /// The resolved MTGO color description or code.
    pub fn colors(&self) -> Option<&'a str> {
        self.text("COLOR")
    }

    /// The resolved MTGO rarity description or code.
    pub fn rarity(&self) -> Option<&'a str> {
        self.text("RARITY_STATUS")
    }

    /// The effective object-type code, such as `CARD`, `BSTR`, or `FULL`.
    pub fn object_type(&self) -> Option<&'a str> {
        self.text("DIGITAL_OBJECT_TYPE_CODE_STRING")
    }

    /// Whether the effective `IS_FOIL` property indicates a foil object.
    ///
    /// Presence flags, `1`, and `true` mean true; `0`, `false`, and absence mean
    /// false. Textual booleans are compared without ASCII case sensitivity.
    ///
    /// # Errors
    /// Returns [`Error::InvalidValue`] when the property contains another value.
    pub fn is_foil(&self) -> Result<bool, Error> {
        match self.field_ref("IS_FOIL") {
            None => Ok(false),
            Some(ResolvedValue::Flag) => Ok(true),
            Some(ResolvedValue::Text(value))
                if *value == "1" || value.eq_ignore_ascii_case("true") =>
            {
                Ok(true)
            }
            Some(ResolvedValue::Text(value))
                if *value == "0" || value.eq_ignore_ascii_case("false") =>
            {
                Ok(false)
            }
            Some(ResolvedValue::Text(value)) => Err(Error::InvalidValue {
                object: self.id(),
                field: "IS_FOIL".to_owned(),
                value: (*value).to_owned(),
            }),
            Some(ResolvedValue::Fields(_)) => Err(Error::InvalidValue {
                object: self.id(),
                field: "IS_FOIL".to_owned(),
                value: "nested fields".to_owned(),
            }),
        }
    }
}

/// A resolved object whose effective object-type code is `CARD`.
///
/// Common field accessors are available through [`Deref`] to [`ResolvedObject`].
#[derive(Debug, Clone)]
pub struct CardView<'a> {
    /// The resolved card record and its inherited source properties.
    object: ResolvedObject<'a>,
}

impl<'a> CardView<'a> {
    /// The complete resolved object behind this card-specific view.
    pub fn object(&self) -> &ResolvedObject<'a> {
        &self.object
    }
}

impl<'a> Deref for CardView<'a> {
    type Target = ResolvedObject<'a>;

    fn deref(&self) -> &Self::Target {
        &self.object
    }
}

/// Read numeric clone targets, accepting both current literals and `DOC_` IDs.
fn clone_id(object: &DigitalObject) -> Result<Option<CatalogId>, Error> {
    let Some(value) = object.fields.get("CLONE_ID") else {
        return Ok(None);
    };
    match value {
        FieldValue::Literal(value) => value
            .strip_prefix("DOC_")
            .unwrap_or(value)
            .parse()
            .map(Some)
            .map_err(|_| Error::InvalidValue {
                object: object.id,
                field: "CLONE_ID".to_owned(),
                value: value.clone(),
            }),
        FieldValue::Reference(value) => Err(Error::InvalidValue {
            object: object.id,
            field: "CLONE_ID".to_owned(),
            value: value.clone(),
        }),
        FieldValue::Flag => Err(Error::InvalidValue {
            object: object.id,
            field: "CLONE_ID".to_owned(),
            value: "presence flag".to_owned(),
        }),
        FieldValue::Fields(_) => Err(Error::InvalidValue {
            object: object.id,
            field: "CLONE_ID".to_owned(),
            value: "nested fields".to_owned(),
        }),
    }
}
