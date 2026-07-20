//! The cross-package import subsystem for the opt-in fan-out.
//!
//! Fan-out splits the DTO surface into one file per pinned `(package, module)`
//! group (see [`super::fan_out`]). Those `(package, module)` pins are JS/npm
//! coordinates (`@earendil-works/pi-ai`, `../src/ai`) — ILLEGAL Rust idents that
//! are ALSO used verbatim for the on-disk file paths. This module bridges the
//! two so the split output actually COMPILES:
//!
//!  * Target model: ONE Rust crate; each fanned file is a Rust `mod`; a
//!    cross-group reference resolves to `crate::<sanitized-path>::Symbol`.
//!  * The bridge between a verbatim (illegal-ident) on-disk file path and a
//!    valid Rust module ident is the `#[path = "…"] mod <ident>;` attribute,
//!    emitted into a generated ROOT module ([`render_root`]).
//!
//! The npm / `.d.ts` / src-tree-swap JS surface is pi's hand-written concern and
//! is OUT OF SCOPE here: fluessig keeps the verbatim pin strings available (for
//! `#[path]` and that JS surface) but never emits Rust crates or `.d.ts`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::api::{ApiDoc, ApiType};

use super::{
    fan_out, is_string_enum, pinned_group, symbol_groups, EnumDesc, SymbolBinding, SymbolGroup,
};

/// Where a symbol lives once the surface is fanned out: its own pinned
/// `(package, module)` group, or the shared root `common` module (the home for
/// symbols with no group pin — enums, and any un-pinned DTO/union).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GroupKey {
    /// A pinned `(package, module)` group. The strings are kept VERBATIM (the
    /// npm coordinates) — sanitization happens only when projecting to a Rust
    /// module path ([`group_module_path`]).
    Pinned { package: String, module: String },
    /// The shared `common` module at the crate root.
    Common,
}

impl GroupKey {
    fn of_group(g: &SymbolGroup) -> Self {
        GroupKey::Pinned {
            package: g.package.clone(),
            module: g.module.clone(),
        }
    }
}

/// The fixed Rust module ident the shared root module lives at.
pub const COMMON_MOD: &str = "common";

/// Sanitize ONE raw path segment to a valid snake_case Rust module ident, or
/// `None` if the segment is a JS relative-path artifact to DROP (`""`, `.`,
/// `..`) or collapses to nothing. Rules: lowercase; each run of non-alphanumeric
/// characters (the npm `@`/`-`/`/` sigils included) collapses to a single `_`;
/// leading/trailing `_` trimmed; a leading digit gets a `_` prefix (Rust idents
/// can't start with a digit). Deterministic and total.
fn sanitize_segment(seg: &str) -> Option<String> {
    if seg.is_empty() || seg == "." || seg == ".." {
        return None;
    }
    let mut out = String::with_capacity(seg.len());
    let mut prev_underscore = false;
    for ch in seg.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        return None;
    }
    let mut ident = trimmed.to_string();
    if ident.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        ident.insert(0, '_');
    }
    Some(ident)
}

/// The Rust module path (under the crate root) a group projects to. `Common`
/// is fixed to `[common]`; a pinned group splits BOTH its `package` and `module`
/// on `/`, drops the JS relative-path artifacts, sanitizes each surviving
/// segment, and nests the module segments under the package segments. Total and
/// deterministic — an all-artifact pin yields an empty path, which
/// [`resolve_module_paths`] rejects.
pub fn group_module_path(key: &GroupKey) -> Vec<String> {
    match key {
        GroupKey::Common => vec![COMMON_MOD.to_string()],
        GroupKey::Pinned { package, module } => package
            .split('/')
            .chain(module.split('/'))
            .filter_map(sanitize_segment)
            .collect(),
    }
}

/// A pinned group paired with its resolved Rust module path.
pub type ResolvedGroup = (SymbolGroup, Vec<String>);

/// Project every pinned group to its Rust module path, detecting COLLISIONS:
/// two DISTINCT `(package, module)` pins mapping to the same Rust path is a hard
/// error (they must NOT be silently merged). The `common` module path is
/// reserved, so a pin that sanitizes onto it also errors. An empty projection
/// (a pin made entirely of dropped artifacts) errors too. Deterministic:
/// preserves the input group order.
pub fn resolve_module_paths(groups: &[SymbolGroup]) -> Result<Vec<ResolvedGroup>, String> {
    // path -> the (package, module) that claimed it, for a clear collision msg.
    let mut claimed: HashMap<Vec<String>, (String, String)> = HashMap::new();
    claimed.insert(
        vec![COMMON_MOD.to_string()],
        ("<reserved: common>".to_string(), String::new()),
    );
    let mut out = Vec::with_capacity(groups.len());
    for g in groups {
        let path = group_module_path(&GroupKey::of_group(g));
        if path.is_empty() {
            return Err(format!(
                "fan-out pin (package={:?}, module={:?}) sanitizes to an EMPTY Rust module path \
                 (every segment was a dropped relative-path artifact)",
                g.package, g.module
            ));
        }
        if let Some((ppkg, pmod)) = claimed.get(&path) {
            return Err(format!(
                "fan-out cross-package COLLISION: pins ({ppkg}, {pmod}) and ({}, {}) both map to \
                 the Rust module path `crate::{}` — distinct pins must not merge; disambiguate one \
                 of the (package, module) coordinates",
                g.package,
                g.module,
                path.join("::")
            ));
        }
        claimed.insert(path.clone(), (g.package.clone(), g.module.clone()));
        out.push((g.clone(), path));
    }
    Ok(out)
}

/// A `symbolName -> GroupKey` map for ALL symbols — models, unions, ops AND
/// enums. This is the home-group index that fixes the enum-duplication bug: each
/// symbol has exactly ONE home, so an enum is emitted ONCE (in its home) and
/// imported everywhere else.
pub type GroupTable = HashMap<String, GroupKey>;

/// Build the [`GroupTable`] for `lang`: a model/union/op is homed by its
/// `bindings[lang]` group pin (else `Common`); an enum has no group-pin channel
/// today ([`EnumDesc`] carries none) so its home is always `Common`.
pub fn group_table(api: &ApiDoc, enums: &[EnumDesc], lang: &str) -> GroupTable {
    let home = |bindings: &BTreeMap<String, SymbolBinding>| match pinned_group(bindings, lang) {
        Some((package, module)) => GroupKey::Pinned { package, module },
        None => GroupKey::Common,
    };
    let mut table = GroupTable::new();
    for m in &api.models {
        table.insert(m.name.clone(), home(&m.bindings));
    }
    for u in &api.unions {
        table.insert(u.name.clone(), home(&u.bindings));
    }
    for i in &api.interfaces {
        for op in &i.ops {
            table.insert(op.name.clone(), home(&op.bindings));
        }
    }
    for (name, _) in enums {
        table.entry(name.clone()).or_insert(GroupKey::Common);
    }
    table
}

/// One cross-group reference to import: the referenced `symbol` and the Rust
/// `module_path` it lives at (so the caller emits `use crate::<path>::<symbol>`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExternalRef {
    pub symbol: String,
    pub module_path: Vec<String>,
}

/// Collect the concrete-type symbol names a type mentions that need importing:
/// `Model` refs and NON-string `Enum` refs, recursing through `List`/`Nullable`.
///
/// A `Union` at a REFERENCE site (a DTO field / op return typed as a union) is
/// deliberately NOT collected: with #40's structured projection, a union that is
/// LOCAL to the file lowers to a `Either{N}<…>` / `{Union}Union` over the file's
/// own tagged structs (no cross-group name), and a union NOT local to the file
/// has no `structured_union` match and falls back to the JSON `String` envelope
/// — so a union reference never yields a cross-group import. The union's own
/// member models/enums (which its tagged structs DO name as bare Rust idents)
/// are walked separately, at the union DEFINITION site (see [`external_refs`]).
/// String-enums (→ `String`) and scalars carry no dependency either.
fn collect_type_refs(api: &ApiDoc, t: &ApiType, out: &mut Vec<String>) {
    match t {
        ApiType::Model { model } => out.push(model.clone()),
        ApiType::Enum { r#enum } => {
            if !is_string_enum(api, r#enum) {
                out.push(r#enum.clone());
            }
        }
        ApiType::List { list } => collect_type_refs(api, list, out),
        ApiType::Nullable { nullable } => collect_type_refs(api, nullable, out),
        ApiType::Scalar(_) | ApiType::Union { .. } => {}
    }
}

/// The set of cross-group references a fanned sub-document makes — the pure
/// heart of the subsystem. Walks EVERY reference site exhaustively (DTO field
/// types, union member types, op params/returns) and returns the refs whose home
/// group ≠ `home`.
///
/// #40 (structured discriminated unions, default-on) made a union DEFINITION name
/// its member types as REAL Rust idents: each tagged variant struct inlines its
/// member model's fields and emits `impl From<Member> for {Union}{Tag}`. So a
/// group file that DEFINES a union references that union's member models/enums —
/// hence the [`ApiDoc::unions`] walk below. (Node/python's projection additionally
/// require the member models to be LOCAL to the union's file — they inline the
/// member's fields — so member models are expected to be co-grouped with their
/// union; when they are, this walk still correctly imports any FURTHER-out types
/// those member fields reach.)
///
/// The op-layer walk (params/returns, which also drive the core-trait signatures)
/// is kept exhaustive EVEN THOUGH [`super::fan_out`] drops ops from a group
/// sub-document today — so this survives a future op-layer fan-out with no change.
pub fn external_refs(sub: &ApiDoc, home: &GroupKey, table: &GroupTable) -> BTreeSet<ExternalRef> {
    let mut names: Vec<String> = Vec::new();
    for m in &sub.models {
        for f in &m.fields {
            collect_type_refs(sub, &f.ty, &mut names);
        }
    }
    // Union DEFINITIONS name their member types as bare idents (#40): walk them.
    for u in &sub.unions {
        for v in &u.variants {
            collect_type_refs(sub, &v.ty, &mut names);
        }
    }
    // Does not fire yet (fan_out drops ops), but kept exhaustive for the op layer.
    for i in &sub.interfaces {
        for op in &i.ops {
            collect_type_refs(sub, &op.returns, &mut names);
            for p in &op.params {
                collect_type_refs(sub, &p.ty, &mut names);
            }
        }
    }
    let mut refs = BTreeSet::new();
    for name in names {
        if let Some(key) = table.get(&name) {
            if key != home {
                refs.insert(ExternalRef {
                    symbol: name,
                    module_path: group_module_path(key),
                });
            }
        }
    }
    refs
}

/// A baseline EXTERNAL-crate import a generated file needs regardless of fan-out
/// — e.g. the shared streaming contract `use fluessig_runtime::{Poll,
/// PollStream};` every backend's prelude opens with. Carried alongside the
/// intra-crate [`ExternalRef`] cross-group imports so EVERY generated `use` line
/// flows through this one import-emission module (rather than a raw string baked
/// into each backend prelude). Rendered as one grouped `use <crate>::{items};`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExternalImport {
    /// The external crate path root (e.g. `fluessig_runtime`).
    pub crate_path: &'static str,
    /// The imported items, in the exact order they should appear in the braces.
    pub items: &'static [&'static str],
}

impl ExternalImport {
    /// The single `use <crate>::{a, b};` line (no trailing newline). A one-item
    /// import still braces (`use c::{X};`) — callers here always pass ≥2 items.
    pub fn render(&self) -> String {
        format!("use {}::{{{}}};", self.crate_path, self.items.join(", "))
    }
}

/// THE shared streaming-contract import: `use fluessig_runtime::{Poll,
/// PollStream};`. Every backend prelude (node/python/ruby/php) renders THIS
/// through [`ExternalImport::render`] instead of hardcoding the string, so the
/// runtime import and the cross-group `use crate::…` imports share one emission
/// path. Byte-identical to the previous raw prelude literal by construction.
pub const RUNTIME_STREAM_IMPORT: ExternalImport = ExternalImport {
    crate_path: "fluessig_runtime",
    items: &["Poll", "PollStream"],
};

/// The dedup'd `use crate::…::Symbol;` block a group file opens with, or the
/// empty string when the file has no cross-group refs (⇒ nothing spliced, output
/// stays byte-identical to the un-imported render). node/python/ruby/php all
/// emit Rust, so the import line is uniform; mcp differs only by restricting
/// its refs to `Model`s (its enums lower to `String`) — see [`fan_out_crate`].
///
/// This module is the SINGLE source of every generated `use` line: the baseline
/// external-crate imports ([`ExternalImport`], emitted inline in each backend's
/// prelude) and the intra-crate cross-group refs (this function, spliced into
/// fanned group files by [`fan_out_crate`]). Two emission SITES — the prelude
/// keeps the runtime import at its exact byte position for single-file parity,
/// and cross-group imports only exist in fan-out — but one rendering path.
pub fn render_use_block(refs: &BTreeSet<ExternalRef>) -> String {
    let mut s = String::new();
    for r in refs {
        s.push_str(&format!(
            "use crate::{}::{};\n",
            r.module_path.join("::"),
            r.symbol
        ));
    }
    s
}

/// The inner-attribute line every backend emits verbatim right before its body;
/// the import block splices in immediately after it (a valid position for `use`).
const SPLICE_ANCHOR: &str = "#![allow(clippy::all)]\n";

/// Splice a rendered use-block into an already-rendered backend file, just after
/// its `#![allow(clippy::all)]` inner attribute. Empty block ⇒ untouched.
fn splice_imports(rendered: String, use_block: &str) -> String {
    if use_block.is_empty() {
        return rendered;
    }
    match rendered.find(SPLICE_ANCHOR) {
        Some(i) => {
            let at = i + SPLICE_ANCHOR.len();
            format!("{}\n{}{}", &rendered[..at], use_block, &rendered[at..])
        }
        // Backend banner shape changed unexpectedly — prepend rather than lose it.
        None => format!("{use_block}\n{rendered}"),
    }
}

/// One node to place in the generated root module tree: a Rust `module_path`
/// (sanitized idents) bound, at its leaf, to a verbatim on-disk `file_path` via
/// `#[path = "…"]`.
#[derive(Clone, Debug)]
pub struct ModEntry {
    pub module_path: Vec<String>,
    pub file_path: String,
}

/// Render the nested `#[path = "…"] mod <ident>;` tree from a set of entries,
/// merging shared path prefixes into inline `pub mod` blocks. The leaf binds to
/// the verbatim file path — the bridge that lets illegal-ident JS paths live on
/// disk while the crate sees a valid module tree.
pub fn render_mod_tree(entries: &[ModEntry]) -> String {
    let refs: Vec<&ModEntry> = entries.iter().collect();
    render_mod_level(&refs, 0, 0)
}

fn render_mod_level(entries: &[&ModEntry], depth: usize, indent: usize) -> String {
    let pad = "    ".repeat(indent);
    // Partition by the segment at `depth`, preserving first-seen order.
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, Vec<&ModEntry>> = HashMap::new();
    for e in entries {
        let seg = e.module_path[depth].clone();
        if !buckets.contains_key(&seg) {
            order.push(seg.clone());
        }
        buckets.entry(seg).or_default().push(*e);
    }
    let mut out = String::new();
    for seg in order {
        let group = &buckets[&seg];
        let leaf = group.iter().find(|e| e.module_path.len() == depth + 1);
        let deeper: Vec<&ModEntry> = group
            .iter()
            .filter(|e| e.module_path.len() > depth + 1)
            .copied()
            .collect();
        if let Some(leaf) = leaf {
            out.push_str(&format!(
                "{pad}#[path = {:?}]\n{pad}pub mod {seg};\n",
                leaf.file_path
            ));
        } else {
            out.push_str(&format!("{pad}pub mod {seg} {{\n"));
            out.push_str(&render_mod_level(&deeper, depth + 1, indent + 1));
            out.push_str(&format!("{pad}}}\n"));
        }
    }
    out
}

/// Everything the fan-out writes for one language: the per-group files (imports
/// already spliced), the optional shared `common` file, and the single root
/// module that ties them into one crate.
pub struct FannedCrate {
    /// `(on-disk path, content)` for each pinned group file.
    pub group_files: Vec<(String, String)>,
    /// `(on-disk path, content)` for the shared `common` module, when non-empty.
    pub common_file: Option<(String, String)>,
    /// `(on-disk path, content)` for the generated root module.
    pub root_file: (String, String),
}

/// The type of the per-backend render closure: `(sub-doc, enums) -> source`.
pub type RenderFn<'a> = dyn Fn(&ApiDoc, &[EnumDesc]) -> String + 'a;

/// Where a fan-out writes, and how it lowers, for one language. Bundles the
/// output-path + mode inputs to [`fan_out_crate`] (keeping its arity sane).
pub struct FanOutSpec<'a> {
    /// The backend language slug (`node`/`python`/`ruby`/`php`/`mcp`).
    pub lang: &'a str,
    /// The group-file path pattern (`{package}`/`{module}` substituted verbatim).
    pub pattern: &'a str,
    /// The generated root module's on-disk path.
    pub mod_out: &'a str,
    /// The shared `common` module's on-disk path (see [`common_path_for`]).
    pub common_out: &'a str,
    /// The optional banner note threaded into every rendered file.
    pub note: Option<&'a str>,
    /// `true` for the mcp backend: restrict a group's imports to `Model` refs
    /// (mcp lowers enums to `String`, so it never imports an enum type).
    pub mcp_models_only: bool,
}

/// Produce the whole fanned crate for `spec.lang`: the group files (each carrying
/// its own DTO surface + the `use crate::…` imports for its cross-group refs), the
/// shared `common` file (enums + any un-pinned DTO/union, emitted ONCE), and the
/// root module (`#[path]` mod-tree + `pub use` re-exports + Python `register()`
/// re-collection). Returns `Ok(None)` when the schema carries no group pins for
/// the language (the caller stays on the single-file path). Propagates the
/// collision/empty-path error from [`resolve_module_paths`].
///
/// `render` renders one sub-document with a given enum slice; passing `&[]` for a
/// pinned group is what dedups enums into `common` (they are never re-emitted per
/// group).
pub fn fan_out_crate(
    api: &ApiDoc,
    enums: &[EnumDesc],
    spec: &FanOutSpec<'_>,
    render: &RenderFn<'_>,
) -> Result<Option<FannedCrate>, String> {
    let FanOutSpec {
        lang,
        pattern,
        mod_out,
        common_out,
        note,
        mcp_models_only,
    } = *spec;
    let groups = symbol_groups(api, lang);
    if groups.is_empty() {
        return Ok(None);
    }
    let resolved = resolve_module_paths(&groups)?;
    let table = group_table(api, enums, lang);

    // The shared `common` surface: every un-pinned DTO/union + ALL enums, once.
    let common_home = GroupKey::Common;
    let common_models: Vec<_> = api
        .models
        .iter()
        .filter(|m| table.get(&m.name) == Some(&common_home))
        .cloned()
        .collect();
    let common_unions: Vec<_> = api
        .unions
        .iter()
        .filter(|u| table.get(&u.name) == Some(&common_home))
        .cloned()
        .collect();
    let has_common = !common_models.is_empty() || !common_unions.is_empty() || !enums.is_empty();

    // mcp imports `Model` refs only — it lowers enums to `String`, so an enum
    // never needs importing there. (external_refs already excludes string-enums;
    // this drops the remaining non-string enums for the mcp backend.)
    let refs_for = |sub: &ApiDoc, home: &GroupKey| -> BTreeSet<ExternalRef> {
        let mut refs = external_refs(sub, home, &table);
        if mcp_models_only {
            refs.retain(|r| !enums.iter().any(|(n, _)| n == &r.symbol));
        }
        refs
    };

    let mut mod_entries: Vec<ModEntry> = Vec::new();

    // ── common file ──
    let common_file = if has_common {
        let sub = ApiDoc {
            fluessig: api.fluessig.clone(),
            source: api.source.clone(),
            models: common_models,
            unions: common_unions,
            interfaces: Vec::new(),
        };
        let refs = refs_for(&sub, &common_home);
        let content = splice_imports(render(&sub, enums), &render_use_block(&refs));
        mod_entries.push(ModEntry {
            module_path: vec![COMMON_MOD.to_string()],
            file_path: common_out.to_string(),
        });
        Some((common_out.to_string(), content))
    } else {
        None
    };

    // ── pinned group files ──
    // fan_out yields (path, sub) aligned with `symbol_groups` order, i.e. with
    // `resolved`; zip them to attach each group's resolved module path.
    let fanned = fan_out(api, lang, pattern);
    let mut group_files = Vec::with_capacity(fanned.len());
    for ((file_path, sub), (group, module_path)) in fanned.into_iter().zip(resolved.iter()) {
        let home = GroupKey::of_group(group);
        let refs = refs_for(&sub, &home);
        // `&[]` enums: enums live in `common`, never re-emitted per group.
        let content = splice_imports(render(&sub, &[]), &render_use_block(&refs));
        mod_entries.push(ModEntry {
            module_path: module_path.clone(),
            file_path: file_path.clone(),
        });
        group_files.push((file_path, content));
    }

    // ── root module ──
    let root = render_root(&mod_entries, lang, has_common, note);
    Ok(Some(FannedCrate {
        group_files,
        common_file,
        root_file: (mod_out.to_string(), root),
    }))
}

/// Assemble the generated root module: the `#[path]` mod-tree, the `pub use`
/// re-exports that preserve the flat FFI export surface across the split, and —
/// for Python — the `register()` re-collection that re-gathers every fragment's
/// classes/functions into the one `#[pymodule]` entry point.
fn render_root(entries: &[ModEntry], lang: &str, has_common: bool, note: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(
        "//! GENERATED by fluessig bindgen (fan-out root). Do not edit.\n\
         //! Ties the fanned per-package group files into one crate: each file is a\n\
         //! Rust `mod` bound to its verbatim on-disk path via `#[path]`, and the flat\n\
         //! export surface is preserved by the `pub use` re-exports below.\n",
    );
    if let Some(n) = note {
        out.push_str(&format!("//! {n}\n"));
    }
    out.push_str("#![allow(clippy::all)]\n\n");

    if lang == "python" {
        out.push_str("use pyo3::prelude::*;\n\n");
    }

    // The `#[path]` mod-tree — the illegal-ident-path ↔ valid-mod bridge.
    out.push_str(&render_mod_tree(entries));
    out.push('\n');

    // Re-exports: preserve the flat surface (every `#[napi]`/`#[pyclass]`/… item
    // visible at the crate root, exactly as the single-file build had it).
    for e in entries {
        out.push_str(&format!("pub use {}::*;\n", e.module_path.join("::")));
    }

    // Python's single `#[pymodule] register()` is fragmented by the split; the
    // root re-collects it by delegating to every fragment's own `register()`.
    if lang == "python" {
        out.push('\n');
        out.push_str(
            "/// Re-collect every fanned fragment's classes/functions into the one\n\
             /// `#[pymodule]` entry point (the split fragments the single register()).\n",
        );
        out.push_str("pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {\n");
        let _ = has_common; // common is one of the entries; delegated below.
        for e in entries {
            out.push_str(&format!(
                "    {}::register(m)?;\n",
                e.module_path.join("::")
            ));
        }
        out.push_str("    Ok(())\n}\n");
    }

    crate::rustfmt::format(out)
}

/// The on-disk path for the shared `common` file, derived as a sibling of the
/// root module path (`<root-dir>/common.rs`). Kept next to the root so a single
/// `--<lang>-mod-out` flag fixes where BOTH the root and its `common` land.
pub fn common_path_for(mod_out: &str) -> String {
    let p = std::path::Path::new(mod_out);
    match p.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => {
            dir.join("common.rs").to_string_lossy().into_owned()
        }
        _ => "common.rs".to_string(),
    }
}
