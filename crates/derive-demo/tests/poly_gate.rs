//! Slice 4 gate: the derive-emitted **polymorphic-family** catalog loads clean
//! through fluessig's existing loader/validator (family membership, the closed
//! leaf set, uniform family key, and the (tag, key) discriminator columns all
//! resolve), and `fluessig-gen`'s physical lowering materialises the shapes as
//! specified — abstract roots have no table, every leaf table shares the family
//! key, and each polymorphic reference lands its tag + key columns.
//!
//! It also proves the *generated* surface (Decision #3): the `<Root>Id` enums are
//! real native sum types — one variant per leaf carrying the family key,
//! heterogeneous across the two families — reachable through the
//! `AbstractRoot::Id` trait alias, and usable in a plain `match`.
//!
//! This is the semantic-equivalence checkpoint from
//! `notes/derive-front-end-decisions.md` (Slice 4): not a byte-for-byte JSON diff
//! but "the derived catalog loads clean through the Rust validator" and drives
//! `fluessig-gen` to the expected output. The byte-level comparison against the
//! TypeSpec emitter is demonstrated out-of-band (`poly.tsp`,
//! `poly_typespec_equivalence.rs`); here we guard the load + projection in CI.

mod common;
use common::col;
use derive_demo::poly::{GhSubject, GhSubjectId, GitObject, GitObjectId};
use fluessig::load_catalog;
use fluessig::sql::{tables, Dialect};
use fluessig_derive::{AbstractRoot, Id};

fn poly() -> fluessig::ir::Catalog {
    let json = derive_demo::poly::fluessig_catalog::to_json();
    load_catalog(&json).expect("Slice 4 derive catalog must load clean")
}

/// The generated `GitObjectId` is a real native sum type: a scalar-keyed family
/// whose variants we construct and `match` over. This is the "prove it's a real
/// enum" check — it wouldn't compile if the derive produced anything but a native
/// Rust `enum` with one variant per leaf carrying the family key (`String`).
#[test]
fn generated_scalar_family_enum_is_a_real_sum_type() {
    fn describe(o: &GitObjectId) -> String {
        match o {
            GitObjectId::Commit(oid) => format!("commit {oid}"),
            GitObjectId::Tree(oid) => format!("tree {oid}"),
            GitObjectId::Blob(oid) => format!("blob {oid}"),
        }
    }

    assert_eq!(describe(&GitObjectId::Commit("abc".into())), "commit abc");
    assert_eq!(describe(&GitObjectId::Tree("def".into())), "tree def");
    assert_eq!(describe(&GitObjectId::Blob("012".into())), "blob 012");

    // it's a real value type: derived PartialEq/Clone hold
    let b = GitObjectId::Blob("012".into());
    assert_eq!(b.clone(), GitObjectId::Blob("012".into()));
    assert_ne!(b, GitObjectId::Tree("012".into()));
}

/// The generated `GhSubjectId` proves **heterogeneous** key handling: a
/// composite-keyed family whose every variant carries `(Id<Repo>, i32)`, not the
/// scalar the other family uses. The `match` is exhaustive over the two leaves,
/// and the variant constructor's type is pinned as `fn(Id<Repo>, i32) ->
/// GhSubjectId` — a compile-time proof of the composite payload without having to
/// build an `Id<Repo>` value.
#[test]
fn generated_composite_family_enum_carries_the_composite_key() {
    fn tag(s: &GhSubjectId) -> &'static str {
        match s {
            GhSubjectId::GhPullRequest(_repo, _number) => "pr",
            GhSubjectId::GhIssue(_repo, _number) => "issue",
        }
    }

    // the variants carry the composite family key (Id<Repo>, i32), heterogeneous
    // vs GitObjectId's scalar String — pinned at the type level.
    let pr_ctor: fn(Id<derive_demo::poly::Repo>, i32) -> GhSubjectId = GhSubjectId::GhPullRequest;
    let issue_ctor: fn(Id<derive_demo::poly::Repo>, i32) -> GhSubjectId = GhSubjectId::GhIssue;
    let _ = (pr_ctor, issue_ctor);

    // and match works once we have a value (Id is a zero-size typed marker)
    let subject = GhSubjectId::GhIssue(Id::default(), 42);
    assert_eq!(tag(&subject), "issue");
}

/// The `AbstractRoot::Id` trait alias makes the conjured `<Root>Id` name
/// discoverable (Decision #3, the go-to-definition answer). `<GitObject as
/// AbstractRoot>::Id` and `<GhSubject as AbstractRoot>::Id` resolve to the two
/// generated enums — checked by round-tripping a value through the alias type.
#[test]
fn abstract_root_alias_names_the_generated_enum() {
    let via_alias: <GitObject as AbstractRoot>::Id = GitObjectId::Commit("z".into());
    assert_eq!(via_alias, GitObjectId::Commit("z".into()));

    let subj_via_alias: <GhSubject as AbstractRoot>::Id =
        GhSubjectId::GhPullRequest(Id::default(), 7);
    assert!(matches!(subj_via_alias, GhSubjectId::GhPullRequest(_, 7)));
}

/// The scalar-keyed family lowers to abstract-root + concrete leaves: `GitObject`
/// is abstract with no table, and `commits` / `trees` / `blobs` each inherit and
/// **share** the family key `oid` (per-leaf key sharing, gate (d)).
#[test]
fn scalar_family_leaves_share_the_key() {
    let c = poly();

    let root = c.entity("GitObject").expect("GitObject present");
    assert!(root.is_abstract, "the family root is abstract");
    assert!(root.table.is_none(), "an abstract root has no table");
    assert_eq!(c.flattened_key(root), vec!["oid"]);

    let t = tables(&c, Dialect::Postgres);
    // the root projects to NO table
    assert!(
        !t.contains_key("git_object") && !t.contains_key("gitobject"),
        "abstract roots don't project to a physical table"
    );
    for leaf in ["commits", "trees", "blobs"] {
        let table = &t[leaf];
        assert_eq!(table.pk, vec!["oid"], "{leaf} shares the family key oid");
        // the inherited family columns land first (oid, then the repo assoc)
        assert_eq!(col(table, "oid").ty, "text");
        assert_eq!(col(table, "repo_id").ty, "text");
    }
    // leaf-specific columns still land
    assert_eq!(col(&t["commits"], "message").ty, "text");
    assert_eq!(col(&t["blobs"], "size").ty, "bigint");
}

/// The composite-keyed family: `GhSubject` abstract, no table, and both leaf
/// tables share the composite key `(repo_id, number)`.
#[test]
fn composite_family_leaves_share_the_composite_key() {
    let c = poly();

    let root = c.entity("GhSubject").expect("GhSubject present");
    assert!(root.is_abstract);
    assert!(root.table.is_none());
    assert_eq!(c.flattened_key(root), vec!["repo_id", "number"]);

    let t = tables(&c, Dialect::Postgres);
    for leaf in ["gh_pull_requests", "gh_issues"] {
        assert_eq!(
            t[leaf].pk,
            vec!["repo_id", "number"],
            "{leaf} shares the composite family key"
        );
    }
}

/// A polymorphic reference materialises the (type-tag, key) column pair. Three
/// sites exercise all the spelling paths: the family default (`git_refs`), a
/// per-site `cols(...)` override (`tree_entries`), and a composite family
/// (`gh_comments`).
#[test]
fn polymorphic_references_land_the_tag_and_key_columns() {
    let c = poly();
    let t = tables(&c, Dialect::Postgres);

    // (a) family-declared spelling: GitObject → (obj_type, obj_oid)
    let refs = &t["git_refs"];
    let names: Vec<&str> = refs.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["id", "obj_type", "obj_oid"]);
    assert_eq!(col(refs, "obj_type").ty, "text"); // discriminator is text
    assert_eq!(col(refs, "obj_oid").ty, "text"); // GitObject key is String

    // (b) per-site override: tree_entries → (entry_type, child_oid)
    let entries = &t["tree_entries"];
    let names: Vec<&str> = entries.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["tree_oid", "name", "entry_type", "child_oid"]);
    assert!(
        !names.contains(&"obj_type") && !names.contains(&"obj_oid"),
        "the per-site cols(...) override replaces the family default spelling"
    );

    // (c) composite family: gh_comments → (subject_type, repo_id, subject_number)
    let comments = &t["gh_comments"];
    let names: Vec<&str> = comments.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["id", "subject_type", "repo_id", "subject_number"]
    );
    assert_eq!(col(comments, "subject_type").ty, "text");
    assert_eq!(col(comments, "repo_id").ty, "text"); // Repo.id is String
    assert_eq!(col(comments, "subject_number").ty, "integer"); // number is i32
}

/// The relations carry the polymorphic wiring in the IR: the discriminator lives
/// in `typeColumn`, the family key spelling in `fkColumns`, and `relation.to`
/// points at the abstract family (which is what makes the validator demand a
/// discriminator in the first place).
#[test]
fn polymorphic_relation_ir_names_the_family_and_discriminator() {
    let c = poly();

    let refs = c.entity("GitRef").unwrap();
    let target = refs.fields.iter().find(|f| f.name == "target").unwrap();
    let rel = target.relation.as_ref().unwrap();
    assert_eq!(rel.to, "GitObject");
    assert_eq!(rel.type_column.as_deref(), Some("obj_type"));
    assert_eq!(
        rel.fk_columns.as_deref(),
        Some(&["obj_oid".to_string()][..])
    );

    let comments = c.entity("GhComment").unwrap();
    let subject = comments
        .fields
        .iter()
        .find(|f| f.name == "subject")
        .unwrap();
    let rel = subject.relation.as_ref().unwrap();
    assert_eq!(rel.to, "GhSubject");
    assert_eq!(rel.type_column.as_deref(), Some("subject_type"));
    assert_eq!(
        rel.fk_columns.as_deref(),
        Some(&["repo_id".to_string(), "subject_number".to_string()][..])
    );
}
