import "./decorators.js";
using TypeSpec.Reflection;

// ═════════════════════════════════════════════════════════════════════════════
// @fluessig/typespec — the fluessig decorator library (plan.txt Step 0).
//
// Import + `using Fluessig;` and everything is bare: @entity, @edge, @compose…
// Primary keys use TypeSpec's BUILT-IN `@key` (no fluessig declaration — ours
// would collide and the built-in's semantics match), extended by fluessig rule
// to be legal on relation fields (FK-in-PK) and on edge-struct fields (local
// keys). See FINDINGS.md #1/#3.
// ═════════════════════════════════════════════════════════════════════════════
namespace Fluessig;

// ── Layer B: the relation vocabulary (DESIGN §3) ──

/** Mark a stored entity (table / collection / node label). */
extern dec entity(target: Model);

/** An abstract polymorphic family root: carries the shared key (+ relations); only leaves instantiate. */
extern dec abstract(target: Model);

/** Attach edge properties to a relation: `props` is a plain model whose fields ride on the edge. */
extern dec edge(target: ModelProperty, props: Model);

/** Composition (embed intent): the target's identity & lifecycle belong to the parent. Default is association. */
extern dec compose(target: ModelProperty);

/** Override the physical name (table for models/relations, column for fields). */
extern dec name(target: unknown, tableOrColumn: valueof string);

/** Target-side FK column name(s); `typeColumn` names the polymorphic discriminator (FINDINGS #2). */
extern dec fk(target: ModelProperty, names: valueof string[], typeColumn?: valueof string);

/** Source-side column name(s) on an association/edge table; `typeColumn` names the
 * discriminator when the SOURCE is an abstract family (FINDINGS #2). */
extern dec fkSource(target: ModelProperty, names: valueof string[], typeColumn?: valueof string);

/** DDL DEFAULT — required for byte parity with hand-written templates (FINDINGS #4). */
extern dec defaultValue(target: ModelProperty, value: valueof unknown);

/** A derived field (DESIGN §9.3, v1 slice: exists|count only, virtual projection).
 * `of` names a to-many relation ON THE SAME ENTITY (a string for now — a checked
 * property reference can't self-reference mid-declaration; the Rust validator
 * resolves it); `filter` is literal equality on the relation's edge properties. */
extern dec derived(target: ModelProperty, agg: valueof string, of: valueof string, filter?: valueof unknown);

// ── the binding surface: op shapes (plan.txt Step 5b; see spike/gen.mjs) ──

/** The constructor: opens the handle. */
extern dec ctor(target: Operation);

/** Returns a poll-based stream of the declared return type. */
extern dec stream(target: Operation);

/** Excluded from generation; hand-written in each binding (the escape hatch). */
extern dec manual(target: Operation);

/** Reads only — no observable state change. Flows into MCP `readOnlyHint`. */
extern dec readonly(target: Operation);

/** Destroys or irreversibly alters state. Flows into MCP `destructiveHint`. */
extern dec destructive(target: Operation);

/** Customize a STREAM op's terminal error EVENT (node backend): the discriminator
 * tag field js-name (default `type`) and its value (default `error`), and the
 * reason / message field js-names (default `reason` / `error`) — pi's post-start
 * error shape. Stream-only; the Rust loader rejects it on any other shape. Args
 * are positional scalars (skip from the right): `@streamError("kind", "failure")`. */
extern dec streamError(
  target: Operation,
  tagName?: valueof string,
  tagValue?: valueof string,
  reasonName?: valueof string,
  errorName?: valueof string,
);

// ── stock scalars: shared vocabulary every catalog gets via `using Fluessig` ──
// Stock scalars behave like TypeSpec builtins: they don't appear in the
// catalog's scalars list; the generators know them by name (sql.rs lowers Json
// to jsonb/json/text per dialect).

/** An opaque JSON payload column (query via json_extract / ::JSON). */
scalar Json extends string;
