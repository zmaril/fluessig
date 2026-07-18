// Golden test for the emitter: run it on the full entl catalog, validate both
// artifacts against the frozen JSON Schemas, and assert the entl invariants
// (the 28-table accounting). The seed of the fixture corpus (notes/plan.txt Step 0).
import { emit } from "./emit.mjs";
import { Ajv2020 } from "ajv/dist/2020.js";
import { createRequire } from "module";
import { fileURLToPath } from "url";
import { dirname, resolve } from "path";
import { mkdtempSync } from "fs";
import { tmpdir } from "os";

const require = createRequire(import.meta.url);
const dir = dirname(fileURLToPath(import.meta.url));

let failures = 0;
const check = (cond, msg) => {
  if (cond) console.log(`  ok  ${msg}`);
  else {
    console.error(`  FAIL ${msg}`);
    failures++;
  }
};

const ajv = new Ajv2020({ allErrors: true, allowUnionTypes: true });
const validCatalog = ajv.compile(require("./catalog.schema.json"));
const validApi = ajv.compile(require("./api.schema.json"));

// ── the full entl catalog ──
console.log("entl.tsp:");
const { catalog, api } = await emit(resolve(dir, "../entl.tsp"), mkdtempSync(resolve(tmpdir(), "fluessig-")));

check(validCatalog(catalog), `catalog.json matches the schema${validCatalog.errors ? " — " + ajv.errorsText(validCatalog.errors) : ""}`);
check(validApi(api), `api.json matches the schema${validApi.errors ? " — " + ajv.errorsText(validApi.errors) : ""}`);

// 29-table accounting: concrete entity tables + relation tables
const concrete = catalog.entities.filter((e) => !e.abstract);
const entityTables = concrete.map((e) => e.table);
const relTables = [
  ...new Set(
    catalog.entities.flatMap((e) => e.fields.filter((f) => f.relation?.table).map((f) => f.relation.table)),
  ),
];
check(concrete.length === 23, `23 concrete entities (got ${concrete.length})`);
check(relTables.length === 6, `6 relation tables (got ${relTables.length})`);
check(new Set([...entityTables, ...relTables]).size === 29, "29 distinct tables");
check(entityTables.every(Boolean), "every concrete entity names its table (@name)");

// polymorphic families
const gitObject = catalog.entities.find((e) => e.name === "GitObject");
check(gitObject?.abstract === true && gitObject.key.join(",") === "oid", "GitObject: abstract root keyed (oid)");
check(
  catalog.entities.filter((e) => e.extends === "GitObject").map((e) => e.name).sort().join(",") === "Blob,Commit,Tree",
  "GitObject family: Blob, Commit, Tree",
);
const treeEntries = catalog.entities
  .find((e) => e.name === "Tree")
  .fields.find((f) => f.name === "entries").relation;
check(
  treeEntries.to === "GitObject" && treeEntries.typeColumn === "entry_type" && treeEntries.properties === "TreeEntry",
  "tree_entries: polymorphic edge with (entry_type, child_oid) + edge props",
);

// edge local keys
const localKeys = Object.fromEntries(
  catalog.relationProperties.map((r) => [r.name, r.fields.filter((f) => f.key).map((f) => f.name)]),
);
check(localKeys.CommitParent?.join(",") === "idx", "commit_parents local key: idx");
check(localKeys.TreeEntry?.join(",") === "name", "tree_entries local key: name");

// enums carry wire values
const fs = catalog.enums.find((e) => e.name === "FileStatus");
check(fs.variants.find((v) => v.name === "added")?.value === "A", 'FileStatus.added = "A"');

// op surface
const entl = api.interfaces.find((i) => i.name === "Entl");
const shapes = Object.fromEntries(entl.ops.map((o) => [o.name, o.shape]));
check(shapes.open === "ctor" && shapes.changes === "stream" && shapes.driverPlan === "stream" && shapes.watch === "manual", "Entl op shapes (ctor/stream×2/manual)");
check(api.interfaces.find((i) => i.name === "Git")?.ops.length === 6, "Git: 6 stateless helpers");
check(
  entl.ops.find((o) => o.name === "changes").params.find((p) => p.name === "options")?.optional === true,
  "optional params survive lowering",
);

// ── the standalone demo catalog still emits validly ──
console.log("tests/fixtures/entl.tsp:");
const demo = await emit(resolve(dir, "../tests/fixtures/entl.tsp"), mkdtempSync(resolve(tmpdir(), "fluessig-")));
check(validCatalog(demo.catalog), "demo catalog matches the schema");
check(validApi(demo.api), "demo api matches the schema");

// ── the tagged-union fixture (format 1) ──
console.log("tests/fixtures/union.tsp:");
const uni = await emit(resolve(dir, "../tests/fixtures/union.tsp"), mkdtempSync(resolve(tmpdir(), "fluessig-")));
check(validCatalog(uni.catalog), `union catalog matches the schema${validCatalog.errors ? " — " + ajv.errorsText(validCatalog.errors) : ""}`);
check(validApi(uni.api), `union api matches the schema${validApi.errors ? " — " + ajv.errorsText(validApi.errors) : ""}`);
const ep = uni.catalog.unions.find((u) => u.name === "EventPayload");
check(ep?.variants.map((v) => v.tag).join(",") === "message,log,exit", "union variants keep declaration order (tags are the discriminators)");
const evt = uni.catalog.entities.find((e) => e.name === "Event");
check(
  JSON.stringify(evt.fields.find((f) => f.name === "payload").type) === '{"k":"union","name":"EventPayload"}',
  "a union field lowers to a named reference",
);
check(
  JSON.stringify(uni.api.interfaces[0].ops.find((o) => o.name === "emit").params[0].type) === '{"union":"EventPayload"}',
  "a union op param lowers to {union: name}",
);
check(
  ["AgentMessage", "LogLine", "ExitInfo"].every((m) => uni.api.models.some((am) => am.name === m)),
  "union variant models join the referenced closure",
);
// drift guard: the emitted fixture must equal the committed one
const committed = {
  catalog: require("../tests/fixtures/catalog.json"),
  api: require("../tests/fixtures/api.json"),
};
check(JSON.stringify(uni.catalog) === JSON.stringify(committed.catalog), "committed union catalog.json is fresh");
check(JSON.stringify(uni.api) === JSON.stringify(committed.api), "committed union api.json is fresh");

if (failures) {
  console.error(`\n${failures} failure(s)`);
  process.exit(1);
}
console.log("\nall green");
