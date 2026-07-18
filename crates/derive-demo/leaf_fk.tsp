// The TypeSpec an author would write for the SAME graph the Slice 8a Gap-1 derive
// demo (`src/leaf_fk.rs`) authors in Rust: a composite-keyed family (Ticket =
// Bug | Feature) plus a DIRECT foreign key into a concrete leaf
// (`Watch.bug: Bug`). Used for the semantic-equivalence comparison in the Gap-1
// gate: emit this via the TypeSpec path and confirm the derive catalog projects
// to the same physical tables/columns/keys — in particular the composite FK
// columns `(project_id, ticket_seq)` on `watches` — modulo the front-end stamp +
// `source` name (see notes/derive-front-end-decisions.md).
//
// The shape Slice 4's poly demo avoided: a reference to a family LEAF resolves
// its FK columns through `extends` to the abstract root's inherited composite
// key. Unlike a polymorphic reference to the family (which adds a type-tag
// column), a direct leaf FK carries NO discriminator — the concrete type is
// known at the site.

import "../../typespec/lib.tsp";
using Fluessig;

@entity
@name("projects")
model Project {
  @key id: string;
}

// ── Composite-keyed family: Ticket = Bug | Feature, key (project, seq) ──

@entity
@abstract
model Ticket {
  @key
  @fk(#["project_id"])
  project: Project;

  @key seq: int32;
}

@entity
@name("bugs")
model Bug extends Ticket {
  severity: string;
}

@entity
@name("features")
model Feature extends Ticket {
  summary: string;
}

@entity
@name("watches")
model Watch {
  @key id: int64;

  // a DIRECT FK into a composite-keyed leaf: the two inherited key columns,
  // spelled (project_id, ticket_seq) — no discriminator.
  @fk(#["project_id", "ticket_seq"])
  bug: Bug;
}
