//! The README-multiplexing gate: interpolation, `fl:only`/`fl:except`,
//! `fl:each` variant selection, the error surface, and the golden
//! regenerates-identically discipline (a fixture template → committed per-language
//! goldens, byte-for-byte). Mirrors the golden convention in tests/mcp.rs.

use fluessig::readme::{by_slug, render, render_files, Language, ReadmeError, RenderCtx};

const TPL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/readme.tpl.md"
));

fn ctx() -> RenderCtx {
    RenderCtx {
        pkg: "entl".into(),
        catalog_name: Some("entl".into()),
    }
}

fn lang(slug: &str) -> &'static Language {
    by_slug(slug).unwrap()
}

// ── interpolation ─────────────────────────────────────────────────────────────

#[test]
fn interpolation_covers_every_key() {
    let out = render(
        "{{ lang }}|{{ lang.slug }}|{{ lang.fence }}|{{ lang.ext }}|{{ pkg }}|{{ catalog.name }}",
        lang("node"),
        &ctx(),
    )
    .unwrap();
    assert_eq!(out, "Node/Bun|node|typescript|ts|entl|entl\n");
}

#[test]
fn install_substitutes_pkg_per_language() {
    let cases = [
        ("rust", "cargo add entl"),
        ("node", "bun add entl"),
        ("python", "uv pip install entl"),
        ("ruby", "bundle add entl"),
    ];
    for (slug, want) in cases {
        let out = render("{{ lang.install }}", lang(slug), &ctx()).unwrap();
        assert_eq!(out, format!("{want}\n"), "{slug}");
    }
}

#[test]
fn unknown_key_is_an_error() {
    assert_eq!(
        render("{{ bogus }}", lang("rust"), &ctx()).unwrap_err(),
        ReadmeError::UnknownKey("bogus".into())
    );
}

// ── conditional inclusion ─────────────────────────────────────────────────────

#[test]
fn only_includes_just_the_listed_targets() {
    let tpl = "top\n<!-- fl:only rust ruby -->\nGATED\n<!-- fl:end -->\ntail";
    assert_eq!(
        render(tpl, lang("rust"), &ctx()).unwrap(),
        "top\nGATED\ntail\n"
    );
    assert_eq!(
        render(tpl, lang("ruby"), &ctx()).unwrap(),
        "top\nGATED\ntail\n"
    );
    assert_eq!(render(tpl, lang("python"), &ctx()).unwrap(), "top\ntail\n");
}

#[test]
fn except_excludes_the_listed_targets() {
    let tpl = "<!-- fl:except python, node -->\nKEPT\n<!-- fl:end -->";
    assert_eq!(render(tpl, lang("rust"), &ctx()).unwrap(), "KEPT\n");
    assert_eq!(render(tpl, lang("node"), &ctx()).unwrap(), "");
    assert_eq!(render(tpl, lang("python"), &ctx()).unwrap(), "");
}

// ── the multiplexer ───────────────────────────────────────────────────────────

#[test]
fn each_selects_the_matching_section_for_every_language() {
    let tpl = "<!-- fl:each -->\nSHARED\n<!-- fl:lang rust -->\nR\n<!-- fl:lang node -->\nN\n<!-- fl:lang python -->\nP\n<!-- fl:lang ruby -->\nB\n<!-- fl:end -->";
    for (slug, tail) in [("rust", "R"), ("node", "N"), ("python", "P"), ("ruby", "B")] {
        assert_eq!(
            render(tpl, lang(slug), &ctx()).unwrap(),
            format!("SHARED\n{tail}\n"),
            "{slug}"
        );
    }
}

#[test]
fn each_shared_preamble_emits_for_all() {
    let tpl = "<!-- fl:each -->\npre {{ lang }}\n<!-- fl:lang rust -->\nR\n<!-- fl:lang default -->\nD\n<!-- fl:end -->";
    assert_eq!(render(tpl, lang("rust"), &ctx()).unwrap(), "pre Rust\nR\n");
    assert_eq!(render(tpl, lang("ruby"), &ctx()).unwrap(), "pre Ruby\nD\n");
}

#[test]
fn each_falls_back_to_default_then_errors_strictly() {
    let with_default =
        "<!-- fl:each -->\n<!-- fl:lang rust -->\nR\n<!-- fl:lang default -->\nD\n<!-- fl:end -->";
    assert_eq!(render(with_default, lang("python"), &ctx()).unwrap(), "D\n");

    let strict = "<!-- fl:each -->\n<!-- fl:lang rust -->\nR\n<!-- fl:end -->";
    assert_eq!(
        render(strict, lang("python"), &ctx()).unwrap_err(),
        ReadmeError::NoVariant("python".into())
    );
}

// ── error surface ─────────────────────────────────────────────────────────────

#[test]
fn unterminated_block_errors() {
    assert_eq!(
        render(
            "<!-- fl:each -->\n<!-- fl:lang rust -->\nR",
            lang("rust"),
            &ctx()
        )
        .unwrap_err(),
        ReadmeError::UnterminatedBlock("each")
    );
    assert_eq!(
        render("<!-- fl:only rust -->\nx", lang("rust"), &ctx()).unwrap_err(),
        ReadmeError::UnterminatedBlock("only")
    );
}

#[test]
fn stray_end_and_lang_error() {
    assert_eq!(
        render("body\n<!-- fl:end -->", lang("rust"), &ctx()).unwrap_err(),
        ReadmeError::StrayEnd
    );
    assert_eq!(
        render("<!-- fl:lang rust -->\nx", lang("rust"), &ctx()).unwrap_err(),
        ReadmeError::StrayLang
    );
}

// ── golden: regenerates identically ───────────────────────────────────────────

#[test]
fn fixture_renders_match_committed_goldens() {
    let goldens = [
        ("rust", include_str!("fixtures/readme.rust.md")),
        ("node", include_str!("fixtures/readme.node.md")),
        ("python", include_str!("fixtures/readme.python.md")),
        ("ruby", include_str!("fixtures/readme.ruby.md")),
    ];
    for (slug, golden) in goldens {
        let got = render(TPL, lang(slug), &ctx()).unwrap();
        assert_eq!(
            got, golden,
            "{slug} render drifted from tests/fixtures/readme.{slug}.md"
        );
    }
}

#[test]
fn render_files_fans_out_over_lang_pattern() {
    let targets: Vec<&Language> = ["rust", "python"].iter().map(|s| lang(s)).collect();
    let files = render_files(TPL, "out/README.{lang}.md", &targets, &ctx()).unwrap();
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(paths, ["out/README.rust.md", "out/README.python.md"]);
    assert_eq!(files[0].1, include_str!("fixtures/readme.rust.md"));
}

#[test]
fn render_files_single_file_when_no_lang_token() {
    let files = render_files(TPL, "README.md", &[lang("ruby")], &ctx()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].0, "README.md");
    assert_eq!(files[0].1, include_str!("fixtures/readme.ruby.md"));
}
