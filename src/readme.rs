//! README multiplexing — one Markdown *template* with directives, rendered per
//! target language so the same "quickstart" shows Rust vs Python vs Ruby vs
//! Node/Bun code from a single source. Unlike the code generators this output is
//! prose: it is built as plain strings (never routed through `rustfmt`), the
//! same way [`crate::bindgen::mcp`] hand-builds the MCP manifest.
//!
//! The template stays valid Markdown that renders sensibly on GitHub — every
//! directive is an HTML comment alone on its own line, so an unrendered template
//! reads as a single (English) variant with the alternatives tucked in comments.
//!
//! Directive syntax (each on its own line, leading/trailing whitespace allowed):
//! - `{{ key }}` — interpolation. Keys: `lang` (display name), `lang.slug`,
//!   `lang.fence`, `lang.install` (with `{pkg}` already substituted), `lang.ext`,
//!   `pkg`, `catalog.name`. Whitespace inside the braces is flexible.
//! - `<!-- fl:only SLUG [SLUG…] -->` … `<!-- fl:end -->` — keep the block only
//!   for the listed targets. `<!-- fl:except SLUG [SLUG…] -->` — keep for all but
//!   the listed. Slugs are space- or comma-separated.
//! - `<!-- fl:each -->` … `<!-- fl:end -->` — the multiplexer. `<!-- fl:lang SLUG -->`
//!   markers partition the block into per-language sections; content before the
//!   first marker is a shared preamble emitted for every target. Only the section
//!   matching the target (or `default`) is emitted; strict otherwise.
//!
//! Blocks nest; the parser is a line-oriented recursive descent.

use std::fmt;

/// One projection target: a canonical id plus the language-specific surface the
/// template interpolates (fence tag, install one-liner, source extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    /// Canonical id used in directives and `{lang}` output paths.
    pub slug: &'static str,
    /// Display name (`{{ lang }}`).
    pub name: &'static str,
    /// Markdown code-fence tag (`{{ lang.fence }}`).
    pub fence: &'static str,
    /// Install one-liner with a `{pkg}` placeholder (`{{ lang.install }}`).
    pub install: &'static str,
    /// Source-file extension, no dot (`{{ lang.ext }}`).
    pub ext: &'static str,
}

/// The four projection targets, in canonical order (rust, node, python, ruby).
pub fn languages() -> &'static [Language] {
    const LANGS: &[Language] = &[
        Language {
            slug: "rust",
            name: "Rust",
            fence: "rust",
            install: "cargo add {pkg}",
            ext: "rs",
        },
        Language {
            slug: "node",
            name: "Node/Bun",
            fence: "typescript",
            install: "bun add {pkg}",
            ext: "ts",
        },
        Language {
            slug: "python",
            name: "Python",
            fence: "python",
            install: "uv pip install {pkg}",
            ext: "py",
        },
        Language {
            slug: "ruby",
            name: "Ruby",
            fence: "ruby",
            install: "bundle add {pkg}",
            ext: "rb",
        },
    ];
    LANGS
}

/// Look a language up by its canonical slug.
pub fn by_slug(slug: &str) -> Option<&'static Language> {
    languages().iter().find(|l| l.slug == slug)
}

/// The render environment: the package name substituted into `{pkg}` in install
/// strings and exposed as `{{ pkg }}`, plus an optional catalog name (`{{ catalog.name }}`).
#[derive(Debug, Clone)]
pub struct RenderCtx {
    /// Package name for install lines and `{{ pkg }}`.
    pub pkg: String,
    /// Catalog name for `{{ catalog.name }}`; referencing it while unset is an error.
    pub catalog_name: Option<String>,
}

/// A catalog name from a `source` like `entl.tsp` — the stem, `.tsp` stripped.
pub fn catalog_name_from_source(source: &str) -> String {
    source.strip_suffix(".tsp").unwrap_or(source).to_string()
}

/// Everything that can go wrong rendering a template. `Display` gives one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadmeError {
    /// `{{ key }}` names something the renderer doesn't know.
    UnknownKey(String),
    /// `{{ catalog.name }}` used but no catalog name was supplied.
    MissingCatalogName,
    /// A `{{` with no closing `}}` on the line.
    UnterminatedInterpolation(String),
    /// A `fl:only` / `fl:except` / `fl:each` block with no matching `fl:end`.
    UnterminatedBlock(&'static str),
    /// A `fl:end` with no open block.
    StrayEnd,
    /// A `fl:lang` marker outside a `fl:each` block.
    StrayLang,
    /// A directive that needs a slug argument (`fl:only`, `fl:lang`, …) has none.
    MissingSlug(&'static str),
    /// A `fl:<word>` the renderer doesn't recognize.
    UnknownDirective(String),
    /// A `fl:each` has no section for the target and no `default`.
    NoVariant(String),
}

impl fmt::Display for ReadmeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadmeError::UnknownKey(k) => write!(f, "unknown interpolation key: {{{{ {k} }}}}"),
            ReadmeError::MissingCatalogName => {
                write!(
                    f,
                    "template uses {{{{ catalog.name }}}} but no catalog name was supplied"
                )
            }
            ReadmeError::UnterminatedInterpolation(line) => {
                write!(f, "unterminated {{{{ … }}}} on line: {line}")
            }
            ReadmeError::UnterminatedBlock(kind) => {
                write!(f, "unterminated fl:{kind} block (missing fl:end)")
            }
            ReadmeError::StrayEnd => write!(f, "fl:end with no open block"),
            ReadmeError::StrayLang => write!(f, "fl:lang outside a fl:each block"),
            ReadmeError::MissingSlug(d) => write!(f, "fl:{d} needs at least one slug"),
            ReadmeError::UnknownDirective(d) => write!(f, "unknown directive fl:{d}"),
            ReadmeError::NoVariant(slug) => {
                write!(f, "fl:each has no section for '{slug}' and no default")
            }
        }
    }
}

impl std::error::Error for ReadmeError {}

// ── the parse tree ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Line(String),
    Only {
        slugs: Vec<String>,
        body: Vec<Node>,
    },
    Except {
        slugs: Vec<String>,
        body: Vec<Node>,
    },
    Each {
        preamble: Vec<Node>,
        sections: Vec<Section>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Section {
    slug: String,
    body: Vec<Node>,
}

/// A classified directive line (the payload after `fl:`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Directive {
    Only(Vec<String>),
    Except(Vec<String>),
    Each,
    Lang(String),
    End,
}

/// The `fl:` payload of a directive line, or `None` for ordinary text (including
/// non-`fl:` HTML comments, which pass through verbatim).
fn as_directive(line: &str) -> Option<&str> {
    line.trim()
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim()
        .strip_prefix("fl:")
}

fn parse_slugs(rest: &str) -> Vec<String> {
    rest.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Classify one line: `Ok(None)` for text, `Ok(Some(dir))` for a directive,
/// `Err` for a malformed directive.
fn classify(line: &str) -> Result<Option<Directive>, ReadmeError> {
    let Some(payload) = as_directive(line) else {
        return Ok(None);
    };
    let kw = payload.split_whitespace().next().unwrap_or("");
    let rest = payload[kw.len()..].trim();
    let dir = match kw {
        "end" => Directive::End,
        "each" => Directive::Each,
        "only" => {
            let slugs = parse_slugs(rest);
            if slugs.is_empty() {
                return Err(ReadmeError::MissingSlug("only"));
            }
            Directive::Only(slugs)
        }
        "except" => {
            let slugs = parse_slugs(rest);
            if slugs.is_empty() {
                return Err(ReadmeError::MissingSlug("except"));
            }
            Directive::Except(slugs)
        }
        "lang" => {
            let slug = parse_slugs(rest)
                .into_iter()
                .next()
                .ok_or(ReadmeError::MissingSlug("lang"))?;
            Directive::Lang(slug)
        }
        other => return Err(ReadmeError::UnknownDirective(other.to_string())),
    };
    Ok(Some(dir))
}

fn parse(template: &str) -> Result<Vec<Node>, ReadmeError> {
    let lines: Vec<&str> = template.lines().collect();
    let mut i = 0;
    let nodes = parse_seq(&lines, &mut i)?;
    if i < lines.len() {
        // parse_seq only stops on End / Lang / EOF; at the top level either is stray.
        return match classify(lines[i])? {
            Some(Directive::Lang(_)) => Err(ReadmeError::StrayLang),
            _ => Err(ReadmeError::StrayEnd),
        };
    }
    Ok(nodes)
}

/// Parse a run of nodes until an `fl:end` / `fl:lang` (left for the caller) or EOF.
fn parse_seq(lines: &[&str], i: &mut usize) -> Result<Vec<Node>, ReadmeError> {
    let mut nodes = Vec::new();
    while *i < lines.len() {
        match classify(lines[*i])? {
            None => {
                nodes.push(Node::Line(lines[*i].to_string()));
                *i += 1;
            }
            Some(Directive::End) | Some(Directive::Lang(_)) => break,
            Some(Directive::Only(slugs)) => {
                *i += 1;
                let body = parse_seq(lines, i)?;
                consume_end(lines, i, "only")?;
                nodes.push(Node::Only { slugs, body });
            }
            Some(Directive::Except(slugs)) => {
                *i += 1;
                let body = parse_seq(lines, i)?;
                consume_end(lines, i, "except")?;
                nodes.push(Node::Except { slugs, body });
            }
            Some(Directive::Each) => {
                *i += 1;
                let node = parse_each(lines, i)?;
                consume_end(lines, i, "each")?;
                nodes.push(node);
            }
        }
    }
    Ok(nodes)
}

/// Consume the `fl:end` that closes a block. After [`parse_seq`] the cursor sits
/// on an End, a Lang, or EOF — a Lang here is a stray marker, EOF is unterminated.
fn consume_end(lines: &[&str], i: &mut usize, block: &'static str) -> Result<(), ReadmeError> {
    if *i >= lines.len() {
        return Err(ReadmeError::UnterminatedBlock(block));
    }
    match classify(lines[*i])? {
        Some(Directive::End) => {
            *i += 1;
            Ok(())
        }
        Some(Directive::Lang(_)) => Err(ReadmeError::StrayLang),
        _ => Err(ReadmeError::UnterminatedBlock(block)),
    }
}

/// Parse the interior of an `fl:each`: a shared preamble, then one section per
/// `fl:lang` marker. Leaves the cursor on the closing `fl:end`.
fn parse_each(lines: &[&str], i: &mut usize) -> Result<Node, ReadmeError> {
    let preamble = parse_seq(lines, i)?;
    let mut sections = Vec::new();
    loop {
        if *i >= lines.len() {
            return Err(ReadmeError::UnterminatedBlock("each"));
        }
        match classify(lines[*i])? {
            Some(Directive::Lang(slug)) => {
                *i += 1;
                let body = parse_seq(lines, i)?;
                sections.push(Section { slug, body });
            }
            Some(Directive::End) => break,
            // parse_seq only yields End / Lang / EOF; EOF handled above.
            _ => return Err(ReadmeError::UnterminatedBlock("each")),
        }
    }
    Ok(Node::Each { preamble, sections })
}

// ── rendering ────────────────────────────────────────────────────────────────

/// Render a template for one target language.
pub fn render(template: &str, lang: &Language, ctx: &RenderCtx) -> Result<String, ReadmeError> {
    let nodes = parse(template)?;
    render_nodes(&nodes, lang, ctx)
}

fn render_nodes(nodes: &[Node], lang: &Language, ctx: &RenderCtx) -> Result<String, ReadmeError> {
    let mut out = String::new();
    render_into(nodes, lang, ctx, &mut out)?;
    Ok(out)
}

fn render_into(
    nodes: &[Node],
    lang: &Language,
    ctx: &RenderCtx,
    out: &mut String,
) -> Result<(), ReadmeError> {
    for node in nodes {
        match node {
            Node::Line(s) => {
                out.push_str(&interpolate(s, lang, ctx)?);
                out.push('\n');
            }
            Node::Only { slugs, body } => {
                if slugs.iter().any(|s| s == lang.slug) {
                    render_into(body, lang, ctx, out)?;
                }
            }
            Node::Except { slugs, body } => {
                if !slugs.iter().any(|s| s == lang.slug) {
                    render_into(body, lang, ctx, out)?;
                }
            }
            Node::Each { preamble, sections } => {
                render_into(preamble, lang, ctx, out)?;
                let sect = sections
                    .iter()
                    .find(|s| s.slug == lang.slug)
                    .or_else(|| sections.iter().find(|s| s.slug == "default"))
                    .ok_or_else(|| ReadmeError::NoVariant(lang.slug.to_string()))?;
                render_into(&sect.body, lang, ctx, out)?;
            }
        }
    }
    Ok(())
}

/// Substitute every `{{ key }}` on a text line.
fn interpolate(line: &str, lang: &Language, ctx: &RenderCtx) -> Result<String, ReadmeError> {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| ReadmeError::UnterminatedInterpolation(line.to_string()))?;
        let key = after[..end].trim();
        out.push_str(&resolve(key, lang, ctx)?);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn resolve(key: &str, lang: &Language, ctx: &RenderCtx) -> Result<String, ReadmeError> {
    Ok(match key {
        "lang" => lang.name.to_string(),
        "lang.slug" => lang.slug.to_string(),
        "lang.fence" => lang.fence.to_string(),
        "lang.install" => lang
            .install
            .replace("{pkg}", &ctx.pkg)
            .trim_end()
            .to_string(),
        "lang.ext" => lang.ext.to_string(),
        "pkg" => ctx.pkg.clone(),
        "catalog.name" => ctx
            .catalog_name
            .clone()
            .ok_or(ReadmeError::MissingCatalogName)?,
        other => return Err(ReadmeError::UnknownKey(other.to_string())),
    })
}

/// Render a template to one or more `(path, content)` pairs. If `out_pattern`
/// contains `{lang}`, render each of `langs` and substitute the slug into the
/// path; otherwise render the single language in `langs` to `out_pattern`.
pub fn render_files(
    template: &str,
    out_pattern: &str,
    langs: &[&Language],
    ctx: &RenderCtx,
) -> Result<Vec<(String, String)>, ReadmeError> {
    let nodes = parse(template)?;
    let mut out = Vec::new();
    if out_pattern.contains("{lang}") {
        for lang in langs {
            let content = render_nodes(&nodes, lang, ctx)?;
            out.push((out_pattern.replace("{lang}", lang.slug), content));
        }
    } else {
        for lang in langs {
            let content = render_nodes(&nodes, lang, ctx)?;
            out.push((out_pattern.to_string(), content));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> RenderCtx {
        RenderCtx {
            pkg: "entl".into(),
            catalog_name: Some("entl".into()),
        }
    }

    fn rust() -> &'static Language {
        by_slug("rust").unwrap()
    }

    #[test]
    fn registry_has_four_and_lookup_works() {
        assert_eq!(languages().len(), 4);
        for slug in ["rust", "node", "python", "ruby"] {
            assert_eq!(by_slug(slug).unwrap().slug, slug);
        }
        assert!(by_slug("cobol").is_none());
    }

    #[test]
    fn as_directive_classifies() {
        assert_eq!(as_directive("<!-- fl:end -->"), Some("end"));
        assert_eq!(as_directive("  <!--fl:only rust-->  "), Some("only rust"));
        assert_eq!(as_directive("<!-- just a comment -->"), None);
        assert_eq!(as_directive("plain text"), None);
    }

    #[test]
    fn slug_split_on_space_or_comma() {
        assert_eq!(
            parse_slugs("rust, node  python"),
            ["rust", "node", "python"]
        );
    }

    #[test]
    fn interpolation_all_keys() {
        let l = by_slug("python").unwrap();
        let s = render(
            "{{ lang }} {{lang.slug}} {{ lang.fence }} {{ lang.ext }} {{ pkg }} {{ catalog.name }}",
            l,
            &ctx(),
        )
        .unwrap();
        assert_eq!(s, "Python python python py entl entl\n");
    }

    #[test]
    fn install_substitutes_pkg() {
        assert_eq!(
            render("{{ lang.install }}", rust(), &ctx()).unwrap(),
            "cargo add entl\n"
        );
    }

    #[test]
    fn unknown_key_errors() {
        let e = render("{{ nope }}", rust(), &ctx()).unwrap_err();
        assert_eq!(e, ReadmeError::UnknownKey("nope".into()));
    }

    #[test]
    fn missing_catalog_name_errors() {
        let c = RenderCtx {
            pkg: "x".into(),
            catalog_name: None,
        };
        assert_eq!(
            render("{{ catalog.name }}", rust(), &c).unwrap_err(),
            ReadmeError::MissingCatalogName
        );
    }

    #[test]
    fn each_selects_variant_with_preamble() {
        let tpl = "<!-- fl:each -->\npre\n<!-- fl:lang rust -->\nR\n<!-- fl:lang python -->\nP\n<!-- fl:end -->";
        assert_eq!(render(tpl, rust(), &ctx()).unwrap(), "pre\nR\n");
        assert_eq!(
            render(tpl, by_slug("python").unwrap(), &ctx()).unwrap(),
            "pre\nP\n"
        );
    }

    #[test]
    fn each_default_fallback_and_strict() {
        let with_default =
            "<!-- fl:each -->\n<!-- fl:lang rust -->\nR\n<!-- fl:lang default -->\nD\n<!-- fl:end -->";
        assert_eq!(
            render(with_default, by_slug("ruby").unwrap(), &ctx()).unwrap(),
            "D\n"
        );
        let strict = "<!-- fl:each -->\n<!-- fl:lang rust -->\nR\n<!-- fl:end -->";
        assert_eq!(
            render(strict, by_slug("ruby").unwrap(), &ctx()).unwrap_err(),
            ReadmeError::NoVariant("ruby".into())
        );
    }

    #[test]
    fn only_and_except() {
        let only = "a\n<!-- fl:only rust node -->\nkeep\n<!-- fl:end -->\nb";
        assert_eq!(render(only, rust(), &ctx()).unwrap(), "a\nkeep\nb\n");
        assert_eq!(
            render(only, by_slug("python").unwrap(), &ctx()).unwrap(),
            "a\nb\n"
        );
        let except = "<!-- fl:except ruby -->\nx\n<!-- fl:end -->";
        assert_eq!(render(except, rust(), &ctx()).unwrap(), "x\n");
        assert_eq!(
            render(except, by_slug("ruby").unwrap(), &ctx()).unwrap(),
            ""
        );
    }

    #[test]
    fn nested_only_around_each() {
        let tpl = "<!-- fl:only rust python -->\n<!-- fl:each -->\n<!-- fl:lang rust -->\nR\n<!-- fl:lang python -->\nP\n<!-- fl:end -->\n<!-- fl:end -->";
        assert_eq!(render(tpl, rust(), &ctx()).unwrap(), "R\n");
        assert_eq!(render(tpl, by_slug("ruby").unwrap(), &ctx()).unwrap(), "");
    }

    #[test]
    fn error_unterminated_block() {
        assert_eq!(
            render("<!-- fl:only rust -->\nx", rust(), &ctx()).unwrap_err(),
            ReadmeError::UnterminatedBlock("only")
        );
    }

    #[test]
    fn error_stray_end_and_lang() {
        assert_eq!(
            render("x\n<!-- fl:end -->", rust(), &ctx()).unwrap_err(),
            ReadmeError::StrayEnd
        );
        assert_eq!(
            render("<!-- fl:lang rust -->", rust(), &ctx()).unwrap_err(),
            ReadmeError::StrayLang
        );
    }

    #[test]
    fn error_unknown_directive_and_missing_slug() {
        assert_eq!(
            render("<!-- fl:frobnicate -->", rust(), &ctx()).unwrap_err(),
            ReadmeError::UnknownDirective("frobnicate".into())
        );
        assert_eq!(
            render("<!-- fl:only -->\n<!-- fl:end -->", rust(), &ctx()).unwrap_err(),
            ReadmeError::MissingSlug("only")
        );
    }
}
