//! Shared set metadata remains exact when source fragments are deduplicated.

use mtgoxml::carddata::{CardDataFile, CardDatabase, CardSet, Error, SetMetadata};
use mtgoxml::{Application, Deployment};

/// Build one immutable source context without a network dependency.
fn source() -> Application {
    let deployment = br#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
        <assemblyIdentity name="MTGO.application" version="1.2.3.4" />
        <deployment /><dependency><dependentAssembly dependencyType="install" codebase="MTGO.exe.manifest">
        <assemblyIdentity name="MTGO.exe" version="1.2.3.4" />
        </dependentAssembly></dependency></assembly>"#;
    let application = br#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
        <assemblyIdentity name="MTGO.exe" version="1.2.3.4" /><application /></assembly>"#;
    Deployment::from_bytes("https://patch.example.test/MTGO.application", deployment)
        .unwrap()
        .with_application_bytes(application)
        .unwrap()
}

/// Insert an empty source fragment so metadata coverage is independent of cards.
fn insert(
    database: &mut CardDatabase,
    source: &Application,
    code: &str,
    age: Option<u32>,
    kind: Option<&str>,
) {
    database
        .insert(
            source,
            CardDataFile::Set(CardSet {
                id: code.to_owned(),
                age,
                kind: kind.map(str::to_owned),
                objects: Vec::new(),
            }),
        )
        .unwrap();
}

/// Identical fragments share one entry and set codes retain stable ordering.
#[test]
fn matching_fragments_are_deduplicated_and_sorted() {
    let source = source();
    let mut database = CardDatabase::new(&source);
    insert(&mut database, &source, "ZZZ", Some(7), Some("PAPER"));
    insert(&mut database, &source, "AAA", Some(0), Some(""));
    insert(&mut database, &source, "ZZZ", Some(7), Some("PAPER"));

    let metadata = database.set_metadata().unwrap();
    assert_eq!(
        metadata.keys().map(String::as_str).collect::<Vec<_>>(),
        ["AAA", "ZZZ"]
    );
    assert_eq!(
        metadata["ZZZ"],
        SetMetadata {
            age: Some(7),
            kind: Some("PAPER".to_owned())
        }
    );
    assert_eq!(database.sets().len(), 3);
}

/// Optional values remain distinct and no absent values are filled implicitly.
#[test]
fn optional_metadata_preserves_missing_empty_and_zero() {
    let source = source();
    let mut database = CardDatabase::new(&source);
    insert(&mut database, &source, "MISSING", None, None);
    insert(&mut database, &source, "EMPTY", Some(0), Some(""));

    let metadata = database.set_metadata().unwrap();
    assert_eq!(
        metadata["MISSING"],
        SetMetadata {
            age: None,
            kind: None
        }
    );
    assert_eq!(
        metadata["EMPTY"],
        SetMetadata {
            age: Some(0),
            kind: Some(String::new())
        }
    );
}

/// Conflicts fail rather than silently choosing metadata from one source file.
#[test]
fn conflicting_metadata_is_rejected_without_changing_fragments() {
    let cases = [
        ((Some(1), Some("PAPER")), (Some(2), Some("PAPER"))),
        ((None, Some("PAPER")), (Some(0), Some("PAPER"))),
        ((None, None), (None, Some(""))),
        (
            (None, Some("FROM_THE_VAULT")),
            (None, Some("FROM THE VAULT")),
        ),
        ((None, Some("PAPER")), (None, Some("paper"))),
    ];
    for ((first_age, first_kind), (second_age, second_kind)) in cases {
        let source = source();
        let mut database = CardDatabase::new(&source);
        insert(&mut database, &source, "CONFLICT", first_age, first_kind);
        insert(&mut database, &source, "CONFLICT", second_age, second_kind);

        let Error::InvalidData(message) = database.set_metadata().unwrap_err() else {
            panic!("expected conflicting metadata");
        };
        assert!(message.contains("CONFLICT"));
        assert_eq!(database.sets().len(), 2);
        assert_eq!(database.sets().next().unwrap().age, first_age);
        assert_eq!(database.sets().next().unwrap().kind.as_deref(), first_kind);
    }
}

/// No fabricated metadata is returned when no set fragments have been loaded.
#[test]
fn an_empty_database_has_no_set_metadata() {
    let database = CardDatabase::new(&source());
    assert!(database.set_metadata().unwrap().is_empty());
}
