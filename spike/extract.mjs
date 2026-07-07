// ONE compile of entl.tsp → BOTH artifacts:
//   catalog.json — the data model  (Layer A records + Layer B relations)  → schema codecs
//   api.json     — the op surface  (shapes, params, returns)              → binding generator
// The two layers live in one authored document; each extractor reads its own layer.
import { NodeHost, compile, getDoc } from "@typespec/compiler";
import { fileURLToPath } from "url";
import { dirname, resolve } from "path";
import { writeFileSync } from "fs";

const dir = dirname(fileURLToPath(import.meta.url));
const input = resolve(dir, process.argv[2] ?? "entl.tsp");
const outDir = dirname(input);
const program = await compile(NodeHost, input, {});
const errs = program.diagnostics.filter((d) => d.severity === "error");
for (const d of program.diagnostics) console.error(`[${d.severity}] ${d.code}: ${d.message}`);
if (errs.length) process.exit(1);

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

// ---- shared type lowering ----------------------------------------------------
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
    case "Intrinsic":
      return t.name; // "void"
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
      };
      delete f.column; // @name on a relation names its table, not a column
    }
    return f;
  });
}

// ---- catalog.json: the model layer -------------------------------------------
const models = [...global.models.values()];
const edgeStructs = new Set(
  models.flatMap((m) => [...m.properties.values()].map((p) => decoTypeArg(p, "edge")).filter(Boolean)),
);

const catalog = {
  scalars: [...global.scalars.values()].map((s) => ({ name: s.name, base: s.baseScalar?.name })),
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

// ---- api.json: the op layer ---------------------------------------------------
const apiType = (t) => {
  const ty = typeRef(t);
  if (typeof ty === "string") return ty; // void
  if (ty.k === "scalar") return ty.name;
  if (ty.k === "enum") return { enum: ty.name };
  if (ty.k === "ref") return { model: ty.name };
  if (ty.k === "list") return { list: ty.of.k === "scalar" ? ty.of.name : { model: ty.of.name } };
  throw new Error(`unsupported type in API surface: ${JSON.stringify(ty)}`);
};

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
    params: [...op.parameters.properties.values()].map((p) => ({
      name: p.name,
      type: apiType(p.type),
    })),
    returns: apiType(op.returnType),
  })),
}));

// The binding generator needs struct defs for every model an op signature references.
const referenced = new Set(
  interfaces.flatMap((i) =>
    i.ops.flatMap((op) =>
      [...op.params.map((p) => p.type), op.returns].filter((t) => typeof t === "object").map((t) => t.model),
    ),
  ),
);
const api = {
  models: [...catalog.valueStructs, ...catalog.entities].filter((m) => referenced.has(m.name)).map((m) => ({
    name: m.name,
    doc: m.doc ?? null,
    fields: m.fields.map((f) => ({ name: f.name, type: f.type.k === "scalar" ? f.type.name : { model: f.type.name }, nullable: f.nullable })),
  })),
  interfaces,
};

writeFileSync(resolve(outDir, "catalog.json"), JSON.stringify(catalog, null, 2));
writeFileSync(resolve(outDir, "api.json"), JSON.stringify(api, null, 2));
console.log(
  `${input.split("/").pop()} → catalog.json (${catalog.entities.length} entities, ${catalog.relationProperties.length} edge structs, ${catalog.valueStructs.length} value structs)`,
);
console.log(`         → api.json (${interfaces.length} interface(s), ${api.models.length} DTO model(s))`);
