# Findings from authoring the full `entl.tsp`

The acid test from plan.txt: author entl's **complete** catalog — all 28 tables + the full binding
op surface — in the fluessig language *before* freezing the Rust IR. Result: **the language holds.**
Every table, key, relation, and op expressed; it compiles; the extractor lowers it correctly
(`catalog.json` + `api.json`). Accounting: 22 concrete entity tables + 6 relation tables
(commit_parents, tree_entries, gh_pr_commits, gh_requested_reviewers, gh_labeled, gh_assignees)
= 28/28. Three abstract roots (GitObject, GhSubject) carry the polymorphic families.

But it holds only **with four vocabulary additions** discovered under pressure. Each is listed with
its evidence; these feed Step 0 (the decorator package) and Step 1 (the loader/validator rules).

*(Since authored: the decorators were promoted out of the file into `./typespec/` — the
`@fluessig/typespec` package — so `entl.tsp` reads bare (`@entity`, `@edge`, …) via
`using Fluessig`. Primary keys use TypeSpec's **built-in** `@key` rather than a fluessig
declaration: no collision, matching semantics, one less thing we own. Enums carry wire values —
`added: "A"` — so status codes stay readable without changing the stored bytes.)*

## Language changes required (PROPOSED decorators used in entl.tsp)

### 1. `@key` must be legal on **relation** fields (FK-in-PK)
SPEC's old rule — key fields must be scalars — is wrong for real schemas. Evidence: **10+ of entl's
28 tables** put an FK in the primary key: `refs (repo_id, name)`, `gh_pull_requests /
gh_issues (repo_id, number)`, `gh_labels (repo_id, name)`, `gh_events (repo_id, id)`,
`file_changes (commit_oid, path)`, `gh_steps (job_id, number)`, `conflicts`… (Initially the git
family root was over-keyed as (oid, repo) — the PG parity gate corrected it: content hashes are
globally unique, so `commits/trees/blobs` key on oid ALONE and repo is a plain association. The
parity gate catching a modeling error on day one is the system working.) Semantics: `@key` on a relation ⇒ its FK column(s) join the PK
at declaration position.

### 2. `@fk(names, typeColumn?)` + `@fkSource(names)` — relation column naming is a three-part story
One `@name` cannot name a relation's columns. A relation needs, case by case:
- **target-side FK column names**: `author → author_id`; multi-column against composite keys:
  `pr → (repo_id, pr_number)` where the target key column is called `number` — names differ.
- **the polymorphic discriminator column**: `tree_entries.entry_type`, `gh_comments.subject_type`.
- **source-side columns on association/edge tables**: `commit_parents.commit_oid`,
  `gh_pr_commits.(repo_id, pr_number)`, and — the spicy case — an edge FROM an abstract family whose
  source side is itself a (type, key) pair: `gh_labeled.(repo_id, subject_type, subject_number)`.
- **column sharing**: `repo_id` serves simultaneously as a key member and part of two different FK
  pairs (`gh_labeled`: subject side AND label side). Composite FKs may overlap; the loader must
  merge, not duplicate, columns.

### 3. Local keys on edge structs — the (from, to) default PK is **wrong**
`commit_parents`' PK is `(commit_oid, idx)` — from-key + an **edge property** — not
`(commit_oid, parent_oid)`. Same for `tree_entries (tree_oid, name)`. So `@key` on an edge-struct
field means: edge-table PK = source key + local key. Plain join tables without props
(`gh_pr_commits`, `gh_labeled`…) do default to all-columns. Both cases exist in production; both
must be expressible.

### 4. `@defaultValue` is required for byte parity
The templates carry `DEFAULT 0` / `DEFAULT false` on 6 columns (`parent_count`, `is_merge`,
`gpg_signed`, `is_symbolic`, `is_binary`, `is_draft`). Without a default decorator the parity gate
can't pass. (Was already in the planned vocabulary as `@default`; confirmed load-bearing. Named
`@defaultValue` — `default` risks keyword trouble.)

## Model findings (no language change, but Step-1 rules)

### 5. Weak entities vs `@compose` — a real modeling tension
`file_changes` and `gh_steps` are weak entities (identity = owner + local field, no global id).
Authored as top-level entities with FK-in-PK (#1) — faithful to today's schema. But their
*ownership* semantics (a doc store should embed a commit's file changes) is what `@compose`
expresses, and using both a parent-side `@compose` field AND a child-side FK-in-PK would declare
the same relationship twice. v1 punts (entities, association only); the composition story for weak
entities needs a design pass before the Mongo codec. **Notably: `@compose` ended up UNUSED in the
faithful catalog** — entl's relational schema encodes ownership as weak entities. Composition earns
its keep only at doc-store projection time.

### 6. Field inheritance breaks column-order parity — abstract roots should carry keys + relations only
First draft shared title/body/author/timestamps on `GhSubject`; but `gh_issues`' real column order
is `…body, state, author_id…` — inherited-fields-first ordering can't reproduce it. Rule: abstract
roots carry **key fields and relations** (join tables add no columns to leaf tables); data fields
live on the leaves, even when duplicated (PR/Issue each declare title/body/author…). The
inheritance-flattening of keys (`Commit.key = [] `— it lives on `GitObject`) is Step-1 loader work;
the spike extractor doesn't flatten.

### 7. `refs.target_oid` — the polymorphism gap is real and now documented in-schema
A ref targets *any* object (annotated tags!), but today's table has no discriminator column and the
ingest resolves to commits. Authored as `target: Commit` for byte parity with a comment; upgrading
to `target: GitObject` (adds `target_type`) is the known schema fix, one edit away — exactly the
scenario that justified modeling polymorphism in v1.

### 8. Schema inconsistency surfaced: `conflicts.merge_oid` is TEXT (hex)
Every other oid column is blob. Authored faithfully as `string` with a doc note. Candidate cleanup
when the templates are regenerated from the catalog.

### 9. Denormalized pairs: `gh_events.actor_id` + `actor_login`
An association plus a cached scalar of the target's field. Authored as both (assoc + plain column).
Fine, but a `@denormalized(of: …)` annotation could someday derive it. Not v1.

## Op-surface findings (Step 5b inputs)

- The full surface fits the four shapes: 6 stateless repo helpers (`Git` interface, all unary) +
  `Entl` = @ctor open, 6 unary ops, **2 stream ops** (`changes`, `driverPlan` — the second stream
  confirmed the shape generalizes), 1 `@manual` (`watch` — host-callback re-entry, correctly
  excluded from generation).
- Bindgen's type surface needs (already planned, now confirmed with instances): **options-bag
  models** (5), **enums as op types** (`SinkTarget`, `FileStatus`), **list returns**
  (`diffCommits → FileDiff[]`), **list-typed fields** (`tables?: string[]`), **optional params**
  (`options?`) — the extractor currently drops param optionality; the real emitter must keep it.
- `op`, `type` are TypeSpec keywords — backtick-escaped field names work fine (`ChangeBatch.op`,
  `GhUser.type`, `GhEvent.type`).

## Verdict

No IR reshape needed: everything discovered is **vocabulary + loader rules**, not structure — the
entity-graph model (Layers A/B), the polymorphic-family design, and the op shapes all survived
contact with the full real schema. The four PROPOSED decorators (#1–#4) go into Step 0's package;
findings #5/#6 become Step-1 validator rules; #7/#8 are entl schema follow-ups the catalog now
documents. `entl.tsp` + its `catalog.json`/`api.json` are the first fixture corpus entries.
