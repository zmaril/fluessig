//! Slice 6 gate: source spans in loader diagnostics.
//!
//! `notes/derive-front-end-decisions.md` (Slice 6) — "`file!()`/`line!()` and
//! `///` into the descriptor so loader diagnostics point at `.rs` lines the way
//! they point at `.tsp` lines today. (Span quality is a known Rust rough edge —
//! verify the diagnostics actually land on the right line.)"
//!
//! This gate authors a **deliberately-broken schema with the derive** — an
//! entity carrying a dangling `Id<Ghost>` the *loader* (not rustc) is meant to
//! catch — runs it through `validate_with_spans`, and asserts the resulting
//! diagnostic NAMES THE RIGHT `.rs` FILE AND LINE of the offending declaration.
//! The expected line is discovered dynamically from this file's own source
//! (`include_str!`), so the assertion proves the reported line is *exact* — not
//! off-by-N, not pointing at a macro call site generically.
//!
//! It also proves the happy path is unchanged: the Slice 1–5 demos still
//! `validate_with_spans` clean (spans never enter the catalog), and the
//! committed catalog/api JSON is untouched (checked out-of-band by the existing
//! per-slice gates, which still pass).

use fluessig_derive::{validate_with_spans, Entity, Id};

// ── A real entity that is deliberately LEFT OUT of the validated catalog, so a
//    reference to it dangles at load time (a loader error, never a rustc one). ──
/// A ghost entity — a valid `#[derive(Entity)]` type, omitted from the catalog
/// on purpose so `Id<Ghost>` references have nothing to resolve to.
#[derive(Entity)]
pub struct Ghost {
    #[key]
    pub id: i64,
}

/// A broken entity: `ghost` references `Ghost`, which is not in the validated
/// entity list, so the loader flags `Broken.ghost: relation targets unknown
/// entity Ghost`. The `ghost` field's declaration line is what the diagnostic
/// must point at.
#[derive(Entity)]
#[fluessig(name = "broken")]
pub struct Broken {
    #[key]
    pub id: i64,
    pub ghost: Id<Ghost>,
}

/// A leaf declaring `extends` on a root that is not in the catalog — an
/// entity-level diagnostic (`Orphan: extends unknown entity MissingRoot`) whose
/// span must point at the `struct Orphan` declaration line.
#[derive(Entity)]
#[fluessig(extends = MissingRoot)]
pub struct Orphan {
    #[key]
    pub id: i64,
}

/// The 1-based line number of the single source line in THIS file containing
/// `needle`. Panics unless exactly one line matches, so a needle assembled to
/// avoid self-matching pins one declaration precisely.
fn line_containing(needle: &str) -> u32 {
    let src = include_str!("span_gate.rs");
    let hits: Vec<u32> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, _)| i as u32 + 1)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "needle {needle:?} must appear on exactly one source line, found {hits:?}"
    );
    hits[0]
}

#[test]
fn dangling_reference_diagnostic_names_the_rs_field_line() {
    // Validate `Broken` WITHOUT `Ghost` in the entity list ⇒ the loader flags the
    // dangling reference, and the derive front end annotates it with the `.rs`
    // file:line of the `ghost` field.
    let diags = validate_with_spans(
        "broken_demo",
        "0.1.0",
        &[<Broken as Entity>::DESCRIPTOR],
        &[],
    )
    .expect_err("a dangling Id<Ghost> must fail loader validation");

    assert_eq!(diags.len(), 1, "expected exactly one diagnostic: {diags:?}");
    let d = &diags[0];
    let rendered = d.to_string();
    println!("dangling-reference diagnostic: {rendered}");

    // The loader's own wording is preserved…
    assert!(
        d.message.contains("relation targets unknown entity Ghost"),
        "message: {}",
        d.message
    );

    // …and it is located at the `ghost` field's exact `.rs` line. The needle is
    // assembled at runtime so THIS line does not self-match.
    let span = d.span.expect("the dangling reference must carry a span");
    assert!(
        span.file.ends_with("span_gate.rs"),
        "file should be this test's source, got {}",
        span.file
    );
    let expected = line_containing(&format!("pub ghost: {}<{}>", "Id", "Ghost"));
    assert_eq!(
        span.line, expected,
        "diagnostic must land on the `ghost` field's declaration line ({expected}), got {}",
        span.line
    );

    // The rendered form is `file:line: message` — the shape a `.tsp` error takes.
    assert!(
        rendered.contains(&format!("span_gate.rs:{expected}:")),
        "rendered diagnostic should carry the file:line prefix, got {rendered}"
    );
}

#[test]
fn entity_level_diagnostic_names_the_rs_struct_line() {
    let diags = validate_with_spans(
        "orphan_demo",
        "0.1.0",
        &[<Orphan as Entity>::DESCRIPTOR],
        &[],
    )
    .expect_err("extending a missing root must fail loader validation");

    let d = diags
        .iter()
        .find(|d| d.message.contains("extends unknown entity MissingRoot"))
        .expect("expected the extends-unknown-root diagnostic");
    println!("entity-level diagnostic: {d}");

    let span = d
        .span
        .expect("the entity-level diagnostic must carry a span");
    assert!(span.file.ends_with("span_gate.rs"), "file: {}", span.file);
    // needle assembled from fragments so this call site does not self-match.
    let expected = line_containing(&format!("pub struct {}", "Orphan"));
    assert_eq!(
        span.line, expected,
        "entity diagnostic must land on the `struct Orphan` line ({expected}), got {}",
        span.line
    );
}

#[test]
fn happy_path_demos_validate_clean_with_spans() {
    // Every Slice 1–5 demo still loads clean through the SAME loader, now via the
    // spanned entry point — proving spans are inert on the happy path (they never
    // enter the catalog, so a clean schema stays clean).
    for (label, entities, edges) in [
        (
            "user",
            derive_demo::fluessig_catalog::ENTITIES,
            derive_demo::fluessig_catalog::EDGES,
        ),
        (
            "graph",
            derive_demo::graph::fluessig_catalog::ENTITIES,
            derive_demo::graph::fluessig_catalog::EDGES,
        ),
        (
            "advanced",
            derive_demo::advanced::fluessig_catalog::ENTITIES,
            derive_demo::advanced::fluessig_catalog::EDGES,
        ),
        (
            "poly",
            derive_demo::poly::fluessig_catalog::ENTITIES,
            derive_demo::poly::fluessig_catalog::EDGES,
        ),
    ] {
        validate_with_spans(label, "0.1.0", entities, edges)
            .unwrap_or_else(|d| panic!("{label} demo should validate clean, got {d:?}"));
    }
}
