//! Registry adapters. Each shells out to the NATIVE tool for its registry and
//! inherits that tool's own auth — we never handle tokens.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::{run_tool, Outcome, Registry};

/// The staging-dir operations every registry adapter implements.
pub trait Adapter {
    /// Human-readable adapter name for messages.
    fn name(&self) -> &'static str;
    /// Stamp the explicit version into the staged manifest.
    fn stamp_version(&self, staging: &Path, version: &str) -> Result<()>;
    /// Drop `readme` into the staged package as README.md.
    fn place_readme(&self, staging: &Path, readme: &Path) -> Result<()>;
    /// Place prebuilt artifacts into the staged package.
    fn place_artifacts(&self, staging: &Path, artifacts: &[PathBuf]) -> Result<()>;
    /// Run the registry dry-run (or, with `confirm`, the real publish).
    fn run_publish(&self, staging: &Path, confirm: bool) -> Result<Outcome>;
}

/// Pick the adapter for a registry.
pub fn for_registry(registry: Registry) -> Box<dyn Adapter> {
    match registry {
        Registry::Crates => Box::new(CargoAdapter),
        Registry::Npm => Box::new(NpmAdapter),
        Registry::Pypi => Box::new(PypiAdapter),
        Registry::Gems => Box::new(GemsAdapter),
    }
}

/// Copy `readme` to `staging/README.md`.
fn copy_readme(staging: &Path, readme: &Path) -> Result<()> {
    let dst = staging.join("README.md");
    fs::copy(readme, &dst)
        .with_context(|| format!("copying readme {} -> {}", readme.display(), dst.display()))?;
    Ok(())
}

/// Copy each artifact into `staging` (root), preserving file names.
fn copy_artifacts_into(staging: &Path, artifacts: &[PathBuf], subdir: Option<&str>) -> Result<()> {
    let dir = match subdir {
        Some(s) => staging.join(s),
        None => staging.to_path_buf(),
    };
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    for art in artifacts {
        let name = art
            .file_name()
            .with_context(|| format!("artifact has no file name: {}", art.display()))?;
        let dst = dir.join(name);
        fs::copy(art, &dst)
            .with_context(|| format!("copying artifact {} -> {}", art.display(), dst.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Cargo / crates.io
// ---------------------------------------------------------------------------

pub struct CargoAdapter;

/// Stamp `[package].version` in a Cargo.toml string. Bails honestly if the
/// version is absent or workspace-inherited.
pub fn stamp_cargo_toml(contents: &str, version: &str) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = contents.parse().context("parsing Cargo.toml")?;
    let pkg = doc
        .get_mut("package")
        .and_then(|p| p.as_table_like_mut())
        .context("Cargo.toml has no [package] table")?;
    match pkg.get("version") {
        None => bail!(
            "workspace-inherited version cannot be stamped standalone; set it in the workspace root (first-pass limitation)"
        ),
        Some(v) => {
            // { workspace = true }
            if let Some(t) = v.as_table_like() {
                if t.get("workspace")
                    .and_then(|w| w.as_bool())
                    .unwrap_or(false)
                {
                    bail!(
                        "workspace-inherited version cannot be stamped standalone; set it in the workspace root (first-pass limitation)"
                    );
                }
            }
        }
    }
    pkg.insert("version", toml_edit::value(version));
    Ok(doc.to_string())
}

impl Adapter for CargoAdapter {
    fn name(&self) -> &'static str {
        "crates"
    }

    fn stamp_version(&self, staging: &Path, version: &str) -> Result<()> {
        let manifest = staging.join("Cargo.toml");
        let contents = fs::read_to_string(&manifest)
            .with_context(|| format!("reading {}", manifest.display()))?;
        let stamped = stamp_cargo_toml(&contents, version)?;
        fs::write(&manifest, stamped).with_context(|| format!("writing {}", manifest.display()))?;
        Ok(())
    }

    fn place_readme(&self, staging: &Path, readme: &Path) -> Result<()> {
        copy_readme(staging, readme)?;
        // Point [package].readme at it.
        let manifest = staging.join("Cargo.toml");
        let contents = fs::read_to_string(&manifest)?;
        let mut doc: toml_edit::DocumentMut = contents.parse()?;
        if let Some(pkg) = doc.get_mut("package").and_then(|p| p.as_table_like_mut()) {
            pkg.insert("readme", toml_edit::value("README.md"));
        }
        fs::write(&manifest, doc.to_string())?;
        Ok(())
    }

    fn place_artifacts(&self, staging: &Path, artifacts: &[PathBuf]) -> Result<()> {
        copy_artifacts_into(staging, artifacts, None)
    }

    fn run_publish(&self, staging: &Path, confirm: bool) -> Result<Outcome> {
        let manifest = staging.join("Cargo.toml");
        let mut cmd = Command::new("cargo");
        cmd.arg("publish").arg("--allow-dirty");
        if confirm {
            cmd.arg("--manifest-path").arg(&manifest);
            let pretty = format!(
                "cargo publish --allow-dirty --manifest-path {}",
                manifest.display()
            );
            run_tool(&mut cmd, &pretty)?;
            Ok(Outcome::Published { command: pretty })
        } else {
            cmd.arg("--dry-run")
                .arg("--no-verify")
                .arg("--manifest-path")
                .arg(&manifest);
            let pretty = format!(
                "cargo publish --dry-run --allow-dirty --no-verify --manifest-path {}",
                manifest.display()
            );
            run_tool(&mut cmd, &pretty)?;
            Ok(Outcome::DryRun { command: pretty })
        }
    }
}

// ---------------------------------------------------------------------------
// npm
// ---------------------------------------------------------------------------

pub struct NpmAdapter;

/// Set `"version"` in a package.json string, returning pretty-printed JSON.
pub fn stamp_package_json(contents: &str, version: &str) -> Result<String> {
    let mut val: serde_json::Value =
        serde_json::from_str(contents).context("parsing package.json")?;
    let obj = val
        .as_object_mut()
        .context("package.json is not a JSON object")?;
    obj.insert(
        "version".to_string(),
        serde_json::Value::String(version.to_string()),
    );
    let mut out = serde_json::to_string_pretty(&val)?;
    out.push('\n');
    Ok(out)
}

impl Adapter for NpmAdapter {
    fn name(&self) -> &'static str {
        "npm"
    }

    fn stamp_version(&self, staging: &Path, version: &str) -> Result<()> {
        let manifest = staging.join("package.json");
        let contents = fs::read_to_string(&manifest)
            .with_context(|| format!("reading {}", manifest.display()))?;
        let stamped = stamp_package_json(&contents, version)?;
        fs::write(&manifest, stamped)?;
        Ok(())
    }

    fn place_readme(&self, staging: &Path, readme: &Path) -> Result<()> {
        copy_readme(staging, readme)
    }

    fn place_artifacts(&self, staging: &Path, artifacts: &[PathBuf]) -> Result<()> {
        copy_artifacts_into(staging, artifacts, None)
    }

    fn run_publish(&self, staging: &Path, confirm: bool) -> Result<Outcome> {
        let mut cmd = Command::new("npm");
        cmd.arg("publish").current_dir(staging);
        if confirm {
            let pretty = "npm publish".to_string();
            run_tool(&mut cmd, &pretty)?;
            Ok(Outcome::Published { command: pretty })
        } else {
            cmd.arg("--dry-run");
            let pretty = "npm publish --dry-run".to_string();
            run_tool(&mut cmd, &pretty)?;
            Ok(Outcome::DryRun { command: pretty })
        }
    }
}

// ---------------------------------------------------------------------------
// PyPI (uv)
// ---------------------------------------------------------------------------

pub struct PypiAdapter;

/// Set `[project].version` in a pyproject.toml string. Bails if dynamic.
pub fn stamp_pyproject_toml(contents: &str, version: &str) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = contents.parse().context("parsing pyproject.toml")?;
    let project = doc
        .get_mut("project")
        .and_then(|p| p.as_table_like_mut())
        .context("pyproject.toml has no [project] table")?;
    // If `version` is declared dynamic, we can't stamp it.
    if let Some(dynamic) = project.get("dynamic").and_then(|d| d.as_array()) {
        let is_dynamic = dynamic.iter().any(|v| v.as_str() == Some("version"));
        if is_dynamic {
            bail!(
                "pyproject.toml declares version as dynamic; cannot stamp a static version (first-pass limitation)"
            );
        }
    }
    project.insert("version", toml_edit::value(version));
    Ok(doc.to_string())
}

impl Adapter for PypiAdapter {
    fn name(&self) -> &'static str {
        "pypi"
    }

    fn stamp_version(&self, staging: &Path, version: &str) -> Result<()> {
        let manifest = staging.join("pyproject.toml");
        let contents = fs::read_to_string(&manifest)
            .with_context(|| format!("reading {}", manifest.display()))?;
        let stamped = stamp_pyproject_toml(&contents, version)?;
        fs::write(&manifest, stamped)?;
        Ok(())
    }

    fn place_readme(&self, staging: &Path, readme: &Path) -> Result<()> {
        copy_readme(staging, readme)
    }

    fn place_artifacts(&self, staging: &Path, artifacts: &[PathBuf]) -> Result<()> {
        // Prebuilt wheels/sdists go into dist/.
        copy_artifacts_into(staging, artifacts, Some("dist"))
    }

    fn run_publish(&self, staging: &Path, confirm: bool) -> Result<Outcome> {
        // If no artifacts were placed into dist/, build from source.
        let dist = staging.join("dist");
        let has_dist = dist.is_dir()
            && fs::read_dir(&dist)
                .map(|mut it| it.next().is_some())
                .unwrap_or(false);
        if !has_dist {
            let mut build = Command::new("uv");
            build.arg("build").current_dir(staging);
            run_tool(&mut build, "uv build")?;
        }

        let mut cmd = Command::new("uv");
        cmd.arg("publish").current_dir(staging);
        if confirm {
            let pretty = "uv publish".to_string();
            run_tool(&mut cmd, &pretty)?;
            Ok(Outcome::Published { command: pretty })
        } else {
            cmd.arg("--dry-run");
            let pretty = "uv publish --dry-run".to_string();
            run_tool(&mut cmd, &pretty)?;
            Ok(Outcome::DryRun { command: pretty })
        }
    }
}

// ---------------------------------------------------------------------------
// RubyGems (honest: no registry dry-run exists)
// ---------------------------------------------------------------------------

pub struct GemsAdapter;

/// Replace the `.version = "..."` assignment in a gemspec string.
pub fn stamp_gemspec(contents: &str, version: &str) -> Result<String> {
    // Match e.g. `spec.version = "0.1.0"` / `s.version='0.1.0'` (single or
    // double quotes, any leading receiver).
    let mut out = String::with_capacity(contents.len());
    let mut replaced = false;
    for line in contents.lines() {
        if !replaced {
            if let Some(new_line) = replace_version_line(line, version) {
                out.push_str(&new_line);
                out.push('\n');
                replaced = true;
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        bail!("could not find a `.version = \"...\"` assignment in the gemspec");
    }
    Ok(out)
}

/// If `line` is a `<recv>.version = <quote>...<quote>` assignment, return the
/// line with the version replaced; otherwise None.
fn replace_version_line(line: &str, version: &str) -> Option<String> {
    // Find `.version` followed (after whitespace) by `=`.
    let idx = line.find(".version")?;
    let after = &line[idx + ".version".len()..];
    let after_trim = after.trim_start();
    let mut rest = after_trim.strip_prefix('=')?;
    rest = rest.trim_start();
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    // Preserve everything up to and including `.version`, plus original spacing
    // around `=` by rebuilding a canonical form.
    let prefix = &line[..idx + ".version".len()];
    Some(format!("{prefix} = {quote}{version}{quote}"))
}

impl Adapter for GemsAdapter {
    fn name(&self) -> &'static str {
        "gems"
    }

    fn stamp_version(&self, staging: &Path, version: &str) -> Result<()> {
        let gemspec = find_gemspec(staging)?;
        let contents = fs::read_to_string(&gemspec)
            .with_context(|| format!("reading {}", gemspec.display()))?;
        let stamped = stamp_gemspec(&contents, version)?;
        fs::write(&gemspec, stamped)?;
        Ok(())
    }

    fn place_readme(&self, staging: &Path, readme: &Path) -> Result<()> {
        copy_readme(staging, readme)
    }

    fn place_artifacts(&self, staging: &Path, artifacts: &[PathBuf]) -> Result<()> {
        copy_artifacts_into(staging, artifacts, None)
    }

    fn run_publish(&self, staging: &Path, confirm: bool) -> Result<Outcome> {
        // Find a prebuilt .gem, else build one (which validates the gemspec —
        // the closest gems gets to a dry-run).
        let gem_file = match find_gem(staging)? {
            Some(g) => g,
            None => {
                let gemspec = find_gemspec(staging)?;
                let gemspec_name = gemspec
                    .file_name()
                    .and_then(|n| n.to_str())
                    .context("gemspec has no file name")?
                    .to_string();
                let mut build = Command::new("gem");
                build.arg("build").arg(&gemspec_name).current_dir(staging);
                run_tool(&mut build, &format!("gem build {gemspec_name}"))?;
                find_gem(staging)?.context("`gem build` did not produce a .gem file")?
            }
        };
        let gem_name = gem_file
            .file_name()
            .and_then(|n| n.to_str())
            .context("gem file has no name")?
            .to_string();
        let would_run = format!("gem push {gem_name}");

        if confirm {
            let mut cmd = Command::new("gem");
            cmd.arg("push").arg(&gem_name).current_dir(staging);
            run_tool(&mut cmd, &would_run)?;
            Ok(Outcome::Published { command: would_run })
        } else {
            let message = format!(
                "gems: no registry dry-run exists. Built {gem_name} (gemspec validated). \
                 Nothing was pushed."
            );
            Ok(Outcome::StubbedNoDryRun { message, would_run })
        }
    }
}

/// Find the single `*.gemspec` in `dir`.
fn find_gemspec(dir: &Path) -> Result<PathBuf> {
    let mut found = None;
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("gemspec") {
            if found.is_some() {
                bail!("multiple .gemspec files found in {}", dir.display());
            }
            found = Some(path);
        }
    }
    found.with_context(|| format!("no .gemspec found in {}", dir.display()))
}

/// Find the newest `*.gem` in `dir`, if any.
fn find_gem(dir: &Path) -> Result<Option<PathBuf>> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("gem") {
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            match &newest {
                Some((t, _)) if *t >= mtime => {}
                _ => newest = Some((mtime, path)),
            }
        }
    }
    Ok(newest.map(|(_, p)| p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copy_dir;

    #[test]
    fn cargo_stamp_roundtrips_and_sets_version() {
        let input = "[package]\nname = \"foo\"\nversion = \"0.0.1\"\nedition = \"2021\"\n\n[dependencies]\nanyhow = \"1\"\n";
        let out = stamp_cargo_toml(input, "9.9.9").unwrap();
        let doc: toml_edit::DocumentMut = out.parse().unwrap();
        assert_eq!(doc["package"]["version"].as_str(), Some("9.9.9"));
        // Unrelated content preserved.
        assert_eq!(doc["package"]["name"].as_str(), Some("foo"));
        assert_eq!(doc["dependencies"]["anyhow"].as_str(), Some("1"));
    }

    #[test]
    fn cargo_stamp_bails_on_workspace_inherited() {
        let input = "[package]\nname = \"foo\"\nversion = { workspace = true }\n";
        let err = stamp_cargo_toml(input, "1.0.0").unwrap_err();
        assert!(err.to_string().contains("workspace-inherited"), "{err}");
    }

    #[test]
    fn cargo_stamp_bails_on_missing_version() {
        let input = "[package]\nname = \"foo\"\nedition = \"2021\"\n";
        let err = stamp_cargo_toml(input, "1.0.0").unwrap_err();
        assert!(err.to_string().contains("workspace-inherited"), "{err}");
    }

    #[test]
    fn npm_stamp_sets_version() {
        let input = "{\n  \"name\": \"foo\",\n  \"version\": \"0.0.1\"\n}\n";
        let out = stamp_package_json(input, "2.3.4").unwrap();
        let val: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(val["version"], serde_json::json!("2.3.4"));
        assert_eq!(val["name"], serde_json::json!("foo"));
    }

    #[test]
    fn pyproject_stamp_sets_version() {
        let input = "[project]\nname = \"foo\"\nversion = \"0.0.1\"\n";
        let out = stamp_pyproject_toml(input, "5.6.7").unwrap();
        let doc: toml_edit::DocumentMut = out.parse().unwrap();
        assert_eq!(doc["project"]["version"].as_str(), Some("5.6.7"));
    }

    #[test]
    fn pyproject_stamp_bails_on_dynamic() {
        let input = "[project]\nname = \"foo\"\ndynamic = [\"version\"]\n";
        let err = stamp_pyproject_toml(input, "1.0.0").unwrap_err();
        assert!(err.to_string().contains("dynamic"), "{err}");
    }

    #[test]
    fn gemspec_version_line_replaced() {
        let input = "Gem::Specification.new do |spec|\n  spec.name = \"foo\"\n  spec.version = \"0.0.1\"\nend\n";
        let out = stamp_gemspec(input, "8.8.8").unwrap();
        assert!(out.contains("spec.version = \"8.8.8\""), "{out}");
        assert!(out.contains("spec.name = \"foo\""));
    }

    #[test]
    fn gemspec_single_quotes_replaced() {
        let input = "  s.version='1.2.3'\n";
        let out = stamp_gemspec(input, "4.5.6").unwrap();
        assert!(out.contains("s.version = '4.5.6'"), "{out}");
    }

    #[test]
    fn gemspec_bails_when_no_version() {
        let input = "  spec.name = \"foo\"\n";
        let err = stamp_gemspec(input, "1.0.0").unwrap_err();
        assert!(err.to_string().contains("version"), "{err}");
    }

    #[test]
    fn copy_dir_copies_nested_fixture() {
        let src = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(src.path().join("a/b")).unwrap();
        fs::write(src.path().join("top.txt"), "top").unwrap();
        fs::write(src.path().join("a/mid.txt"), "mid").unwrap();
        fs::write(src.path().join("a/b/deep.txt"), "deep").unwrap();

        let dst = tempfile::TempDir::new().unwrap();
        let target = dst.path().join("copy");
        copy_dir(src.path(), &target).unwrap();

        assert_eq!(fs::read_to_string(target.join("top.txt")).unwrap(), "top");
        assert_eq!(fs::read_to_string(target.join("a/mid.txt")).unwrap(), "mid");
        assert_eq!(
            fs::read_to_string(target.join("a/b/deep.txt")).unwrap(),
            "deep"
        );
    }
}
