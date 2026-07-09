//! Slice 1 gate: the derive-emitted catalog loads clean through fluessig's
//! existing Rust loader/validator (the one gate every front end passes through),
//! and the validated IR carries the scalar entity exactly as authored.
//!
//! This is the semantic-equivalence checkpoint from
//! `notes/derive-front-end-decisions.md`: not a byte-for-byte JSON diff, but
//! "the derived catalog loads clean through the Rust validator." The byte-level
//! comparison against the TypeSpec emitter is demonstrated out-of-band in the
//! PR; here we guard the load + validation in CI.

use fluessig::ir::TypeRef;
use fluessig::load_catalog;

#[test]
fn emitted_catalog_loads_and_validates() {
    let json = derive_demo::fluessig_catalog::to_json();
    let catalog = load_catalog(&json).expect("derive-emitted catalog must load clean");

    assert_eq!(catalog.entities.len(), 1);
    let user = &catalog.entities[0];
    assert_eq!(user.name, "User");
    assert_eq!(user.table.as_deref(), Some("users"));
    assert_eq!(user.key, vec!["id".to_string()]);
    assert_eq!(catalog.table_name(user), "users");

    // scalar carriers survive the round-trip
    let by_name = |n: &str| user.fields.iter().find(|f| f.name == n).unwrap();
    match &by_name("id").ty {
        TypeRef::Scalar { name, base } => {
            assert_eq!(name, "int64");
            assert_eq!(base.as_deref(), Some("numeric"));
        }
        other => panic!("id should be a scalar, got {other:?}"),
    }
    assert!(by_name("id").key);
    assert!(!by_name("name").key);

    // Option<T> lowered to nullable
    assert!(by_name("name").nullable);
    assert!(!by_name("login").nullable);

    // doc comments flow into the descriptor
    assert_eq!(by_name("id").doc.as_deref(), Some("The user's unique id."));
}
