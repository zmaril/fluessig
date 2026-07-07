#!/usr/bin/env node
// @fluessig/emitter — the catalog printer (notes/plan.txt Step 0; DESIGN §4).
//
// ONE compile of a .tsp using the fluessig decorators → BOTH artifacts:
//   catalog.json — the data model  (Layer A records + Layer B relations)  → schema codecs
//   api.json     — the op surface  (shapes, params, returns)              → binding generator
//
// Deliberately DUMB: walk the checked program, serialize types + decorator state
// verbatim, stamp versions, exit. No validation, no naming policy, no projection
// logic — all of that lives in the Rust catalog loader so every front-end passes
// through the same validator (DESIGN §4, decision #8).
//
// Usage: fluessig-emit <schema.tsp> [--out <dir>]     (default out: beside the input)
//
// straitjacket-allow-file:duplication — the catalog and api emissions are
// deliberately parallel walks of the same checked program (the dumb-printer
// design, DESIGN §4); sharing more would couple the two layers' shapes.
import { MANIFEST, NodeHost, compile, getDoc } from "@typespec/compiler";
import { createRequire } from "module";
import { resolve, dirname, basename } from "path";
import { writeFileSync } from "fs";

// ── the frozen catalog/api format version (bump on any shape change) ──
// format 1: named tagged unions (`unions` sections; `{k:"union", name}` /
// `{union: name}` type references).
export const FORMAT_VERSION = 1;

const require = createRequire(import.meta.url);
const versions = {
  format: FORMAT_VERSION,
  emitter: require("./package.json").version,
  compiler: MANIFEST.version,
};

export async function emit(inputPath, outDir) {
  const input = resolve(inputPath);
  const out = resolve(outDir ?? dirname(input));
  const program = await compile(NodeHost, input, {});

  const errs = program.diagnostics.filter((d) => d.severity === "error");
  for (const d of program.diagnostics) console.error(`[${d.severity}] ${d.code}: ${d.message}`);
  if (errs.length) throw new Error(`${errs.length} compile error(s) in ${input}`);

  const global = program.getGlobalNamespaceType();
  const decoName = (d) => (d.definition?.name ?? d.decorator?.name ?? "").replace(/^[@$]/, "");
  const hasDeco = (t, n) => (t.decorators ?? []).some((d) => decoName(d) === n);
  const decoJsArgs = (t, n) => {
    const d = (t.decorators ?? []).find((d) => decoName(d) === n);
    return d ? (d.args ?? []).map((a) => a.jsValue ?? a.value?.name ?? null) : null;
  };
  const decoTypeArg = (t, n) => {
    const d = (t.decorators ?? []).find((d) => decoName(d) === n);
    const a = d?.args?.[0];
    return a?.value?.name ?? a?.jsValue ?? null;
  };

  // ---- shared type lowering --------------------------------------------------
  function typeRef(t) {
    if (!t) return null;
    switch (t.kind) {
      case "Scalar": {
        let root = t;
        while (root.baseScalar) root = root.baseScalar;
        return { k: "scalar", name: t.name, base: root.name === t.name ? undefined : root.name };
      }
      case "Model":
        if (t.name === "Array" && t.indexer) return { k: "list", of: typeRef(t.indexer.value) };
        return { k: "ref", name: t.name, entity: hasDeco(t, "entity") };
      case "Enum":
        return { k: "enum", name: t.name };
      case "Union":
        // A NAMED union is a declared tagged union — reference it; its variants
        // live in the catalog/api `unions` section. Anonymous unions survive
        // only as the op layer's `T | null` (lowered in apiTypeOfRef).
        if (t.name) return { k: "union", name: t.name };
        return { k: "union", of: [...t.variants.values()].map((v) => typeRef(v.type)) };
      case "Intrinsic":
        return t.name; // "void" / "null"
      default:
        return { k: t.kind.toLowerCase(), name: t.name };
    }
  }

  const isEntityRef = (ty) =>
    ty?.k === "ref" ? ty.entity : ty?.k === "list" ? ty.of?.k === "ref" && ty.of.entity : false;

  function fields(model) {
    return [...model.properties.values()].map((p) => {
      const ty = typeRef(p.type);
      const f = { name: p.name, type: ty, nullable: !!p.optional };
      const doc = getDoc(program, p);
      if (doc) f.doc = doc;
      if (hasDeco(p, "key")) f.key = true;
      const col = decoJsArgs(p, "name");
      if (col) f.column = col[0];
      const dv = decoJsArgs(p, "defaultValue");
      if (dv) f.default = dv[0];
      const der = decoJsArgs(p, "derived");
      if (der) f.derived = { agg: der[0], of: der[1], filter: der[2] ?? null };
      if (isEntityRef(ty)) {
        const fk = decoJsArgs(p, "fk");
        const src = decoJsArgs(p, "fkSource");
        f.relation = {
          to: ty.k === "list" ? ty.of.name : ty.name,
          cardinality: ty.k === "list" ? "many" : "one",
          kind: hasDeco(p, "compose") ? "composition" : "association",
          properties: decoTypeArg(p, "edge"),
          table: decoJsArgs(p, "name")?.[0],
          fkColumns: fk?.[0] ?? null,
          typeColumn: fk?.[1] ?? null,
          sourceColumns: src?.[0] ?? null,
          sourceTypeColumn: src?.[1] ?? null,
        };
        delete f.column; // @name on a relation names its table, not a column
      }
      return f;
    });
  }

  // ---- catalog.json: the model layer -----------------------------------------
  const models = [...global.models.values()];
  const edgeStructs = new Set(
    models.flatMap((m) => [...m.properties.values()].map((p) => decoTypeArg(p, "edge")).filter(Boolean)),
  );

  const catalog = {
    fluessig: versions,
    source: basename(input),
    scalars: [...global.scalars.values()].map((s) => ({ name: s.name, base: s.baseScalar?.name })),
    // named tagged unions: the variant NAME is the wire discriminator tag
    unions: [...global.unions.values()].map((u) => ({
      name: u.name,
      doc: getDoc(program, u) ?? undefined,
      variants: [...u.variants.values()].map((v) => ({ tag: v.name, type: typeRef(v.type) })),
    })),
    enums: [...global.enums.values()].map((e) => ({
      name: e.name,
      // {name, value}: `value` is the stored wire value when it differs from the name
      variants: [...e.members.values()].map((m) =>
        m.value !== undefined ? { name: m.name, value: m.value } : { name: m.name },
      ),
    })),
    entities: models.filter((m) => hasDeco(m, "entity")).map((m) => ({
      name: m.name,
      table: decoJsArgs(m, "name")?.[0],
      abstract: hasDeco(m, "abstract") || undefined,
      extends: m.baseModel?.name,
      key: [...m.properties.values()].filter((p) => hasDeco(p, "key")).map((p) => p.name),
      doc: getDoc(program, m) ?? undefined,
      fields: fields(m),
    })),
    relationProperties: models.filter((m) => edgeStructs.has(m.name)).map((m) => ({
      name: m.name,
      fields: fields(m),
    })),
    valueStructs: models
      .filter((m) => !hasDeco(m, "entity") && !edgeStructs.has(m.name))
      .map((m) => ({ name: m.name, doc: getDoc(program, m) ?? undefined, fields: fields(m) })),
  };

  // ---- api.json: the op layer --------------------------------------------------
  // one lowering for both entry points: an already-lowered typeRef → an api type
  const apiTypeOfRef = (ty) => {
    if (typeof ty === "string") return ty; // void / null
    if (ty.k === "union") {
      // a NAMED union → a tagged-union reference (variants ride in api.unions)
      if (ty.name) return { union: ty.name };
      // `T | null` → a nullable T (the one anonymous union the op surface supports)
      const parts = ty.of.filter((v) => v !== "null");
      if (parts.length === 1 && ty.of.length === 2) return { nullable: apiTypeOfRef(parts[0]) };
      throw new Error(`unsupported union in API surface: ${JSON.stringify(ty)}`);
    }
    if (ty.k === "scalar") return ty.name;
    if (ty.k === "enum") return { enum: ty.name };
    if (ty.k === "ref") return { model: ty.name };
    if (ty.k === "list") return { list: apiTypeOfRef(ty.of) };
    throw new Error(`unsupported type in API surface: ${JSON.stringify(ty)}`);
  };
  const apiType = (t) => apiTypeOfRef(typeRef(t));

  const interfaces = [...global.interfaces.values()].map((i) => ({
    name: i.name,
    doc: getDoc(program, i) ?? null,
    ops: [...i.operations.values()].map((op) => ({
      name: op.name,
      doc: getDoc(program, op) ?? null,
      shape: hasDeco(op, "ctor") ? "ctor"
           : hasDeco(op, "stream") ? "stream"
           : hasDeco(op, "manual") ? "manual"
           : "unary",
      // MCP tool annotations (readOnlyHint / destructiveHint); omitted when unset
      ...(hasDeco(op, "readonly") ? { readonly: true } : {}),
      ...(hasDeco(op, "destructive") ? { destructive: true } : {}),
      params: [...op.parameters.properties.values()].map((p) => ({
        name: p.name,
        type: apiType(p.type),
        optional: !!p.optional || undefined,
      })),
      returns: apiType(op.returnType),
    })),
  }));

  const referenced = new Set();
  const referencedUnions = new Set();
  // seed from op params/returns (already api-typed: {model} / {union} / {list} / {nullable})
  const seedApiType = (t) => {
    if (typeof t !== "object" || t === null) return;
    if (t.model) referenced.add(t.model);
    if (t.union) referencedUnions.add(t.union);
    if (t.list) seedApiType(t.list);
    if (t.nullable) seedApiType(t.nullable);
  };
  for (const i of interfaces) for (const op of i.ops) [...op.params.map((p) => p.type), op.returns].forEach(seedApiType);
  // transitive: models referenced by referenced models' fields (e.g. SinkOptions.rename
  // -> TableRename), unions referenced by fields, and models referenced by union variants
  const allModels = [...catalog.valueStructs, ...catalog.entities];
  const unionByName = Object.fromEntries(catalog.unions.map((u) => [u.name, u]));
  let grew = true;
  const addTypeRef = (ty) => {
    const inner = ty.k === "list" ? ty.of : ty;
    if (inner.k === "ref" && !referenced.has(inner.name)) {
      referenced.add(inner.name);
      grew = true;
    }
    if (inner.k === "union" && inner.name && !referencedUnions.has(inner.name)) {
      referencedUnions.add(inner.name);
      grew = true;
    }
  };
  while (grew) {
    grew = false;
    for (const m of allModels.filter((m) => referenced.has(m.name)))
      for (const f of m.fields) addTypeRef(f.type);
    for (const u of catalog.unions.filter((u) => referencedUnions.has(u.name)))
      for (const v of u.variants) addTypeRef(v.type);
  }
  const api = {
    fluessig: versions,
    source: basename(input),
    models: [...catalog.valueStructs, ...catalog.entities]
      .filter((m) => referenced.has(m.name))
      .map((m) => ({
        name: m.name,
        doc: m.doc ?? null,
        fields: m.fields.map((f) => ({
          name: f.name,
          type: apiTypeOfRef(f.type),
          nullable: f.nullable,
        })),
      })),
    unions: [...referencedUnions].sort().map((name) => {
      const u = unionByName[name];
      return {
        name: u.name,
        doc: u.doc ?? null,
        variants: u.variants.map((v) => ({ tag: v.tag, type: apiTypeOfRef(v.type) })),
      };
    }),
    interfaces,
  };

  writeFileSync(resolve(out, "catalog.json"), JSON.stringify(catalog, null, 2) + "\n");
  writeFileSync(resolve(out, "api.json"), JSON.stringify(api, null, 2) + "\n");
  return { catalog, api, out };
}

// ── CLI ──
if (process.argv[1] && import.meta.url.endsWith(basename(process.argv[1]))) {
  const args = process.argv.slice(2);
  const outIdx = args.indexOf("--out");
  const outDir = outIdx >= 0 ? args.splice(outIdx, 2)[1] : undefined;
  const input = args[0];
  if (!input) {
    console.error("usage: fluessig-emit <schema.tsp> [--out <dir>]");
    process.exit(2);
  }
  const { catalog, api, out } = await emit(input, outDir);
  console.log(
    `${basename(input)} → ${out}/catalog.json (${catalog.entities.length} entities, ` +
      `${catalog.relationProperties.length} edge structs, ${catalog.valueStructs.length} value structs)`,
  );
  console.log(
    `${" ".repeat(basename(input).length)} → ${out}/api.json (${api.interfaces.length} interface(s), ${api.models.length} DTO model(s))`,
  );
}
