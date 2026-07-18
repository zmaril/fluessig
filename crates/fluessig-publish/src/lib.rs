//! fluessig-publish: a dumb, explicit, commit-style-agnostic publishing tool.
//!
//! It takes an explicit `--version`, explicit inputs (`--path`, `--artifact`,
//! `--readme`), and a target registry, then shells out to the NATIVE tool for
//! that registry — inheriting that tool's own auth. There is NO changeset
//! parsing, NO commit-message parsing, and NO version inference, ever.
//!
//! Dry-run is the default. A real publish happens only when `--confirm` is
//! passed.
//!
//! This crate is deliberately self-contained: it depends only on third-party
//! crates so it can be extracted to its own repo later.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

pub mod adapters;

/// The `fluessig` publishing binary.
#[derive(Debug, Parser)]
#[command(
    name = "fluessig",
    about = "Simple, commit-style-agnostic publishing (dry-run by default)."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Publish a package to a registry (dry-run unless --confirm).
    Publish(PublishArgs),
}

/// The registries we know how to publish to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Registry {
    /// crates.io, via `cargo publish`.
    Crates,
    /// npm, via `npm publish`.
    Npm,
    /// PyPI, via `uv build` + `uv publish`.
    Pypi,
    /// RubyGems, via `gem build` + `gem push`.
    Gems,
}

#[derive(Debug, Parser)]
pub struct PublishArgs {
    /// Target registry.
    #[arg(long = "to", value_enum)]
    pub to: Registry,

    /// The package dir where the manifest lives. Relative to --repo when --ref
    /// is set; otherwise interpreted as-is.
    #[arg(long)]
    pub path: PathBuf,

    /// The explicit version to stamp in. No inference — you say what it is.
    #[arg(long)]
    pub version: String,

    /// Optional git ref (SHA or tag) to publish from. Checked out into an
    /// isolated worktree of --repo; the working tree is never touched.
    #[arg(long = "ref")]
    pub git_ref: Option<String>,

    /// The git repo root used to resolve --ref.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    /// Optional label for messages.
    #[arg(long)]
    pub package: Option<String>,

    /// Prebuilt artifacts to include (repeatable): a .node, a wheel, a .gem, …
    #[arg(long = "artifact")]
    pub artifacts: Vec<PathBuf>,

    /// Optional README to drop into the package as README.md.
    #[arg(long)]
    pub readme: Option<PathBuf>,

    /// Actually publish. Without this flag, everything is a DRY RUN.
    #[arg(long)]
    pub confirm: bool,
}

/// What an adapter's publish step actually did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A real registry dry-run ran and validated the package.
    DryRun {
        /// The command line that was run.
        command: String,
    },
    /// No registry dry-run exists (gems); we validated what we could locally
    /// and describe the command a real publish would run.
    StubbedNoDryRun {
        /// A human-readable explanation of what was validated and what would run.
        message: String,
        /// The command a `--confirm` run would execute.
        would_run: String,
    },
    /// A real publish happened.
    Published {
        /// The command line that was run.
        command: String,
    },
}

/// Entry point used by the binary.
pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Cmd::Publish(args) => publish(args),
    }
}

/// Orchestrate a publish using the safe staging model:
/// resolve source (optionally via an isolated worktree) → copy into a fresh
/// staging tempdir → stamp/readme/artifacts/publish there → clean up.
pub fn publish(args: PublishArgs) -> Result<()> {
    let adapter = adapters::for_registry(args.to);
    let label = args
        .package
        .clone()
        .unwrap_or_else(|| adapter.name().to_string());

    // 1. Resolve the source package dir, optionally via an isolated worktree.
    let mut worktree: Option<Worktree> = None;
    let source_pkg: PathBuf = if let Some(git_ref) = &args.git_ref {
        let wt = Worktree::add(&args.repo, git_ref)
            .with_context(|| format!("adding worktree for ref {git_ref}"))?;
        let pkg = wt.path().join(&args.path);
        worktree = Some(wt);
        pkg
    } else {
        args.path.clone()
    };

    if !source_pkg.is_dir() {
        // Ensure worktree cleanup runs even on this early bail.
        drop(worktree);
        bail!(
            "source package dir does not exist or is not a directory: {}",
            source_pkg.display()
        );
    }

    // 2. Copy the source pkg into a fresh staging tempdir — never mutate source.
    let staging_root = tempfile::TempDir::new().context("creating staging tempdir")?;
    let staging = staging_root.path().join("pkg");
    copy_dir(&source_pkg, &staging)
        .with_context(|| format!("copying {} into staging", source_pkg.display()))?;

    println!("== fluessig publish ==");
    println!("registry : {}", adapter.name());
    println!("package  : {label}");
    println!("version  : {}", args.version);
    println!("source   : {}", source_pkg.display());
    if let Some(git_ref) = &args.git_ref {
        println!("ref      : {git_ref} (isolated worktree)");
    }
    println!("staging  : {}", staging.display());
    println!(
        "mode     : {}",
        if args.confirm {
            "PUBLISH (--confirm)"
        } else {
            "DRY RUN (default)"
        }
    );
    println!();

    // 3. Adapter steps in staging.
    let result = (|| -> Result<Outcome> {
        adapter
            .stamp_version(&staging, &args.version)
            .context("stamping version")?;
        println!("stamped version {} into manifest", args.version);

        if let Some(readme) = &args.readme {
            adapter
                .place_readme(&staging, readme)
                .context("placing readme")?;
            println!("placed readme from {}", readme.display());
        }

        if !args.artifacts.is_empty() {
            adapter
                .place_artifacts(&staging, &args.artifacts)
                .context("placing artifacts")?;
            println!("placed {} artifact(s)", args.artifacts.len());
        }

        adapter.run_publish(&staging, args.confirm)
    })();

    // 4. Clean up the worktree explicitly (TempDirs drop on scope exit).
    if let Some(wt) = worktree {
        if let Err(e) = wt.remove() {
            eprintln!("warning: failed to remove worktree: {e:#}");
        }
    }

    // 5. Summarize.
    let outcome = result?;
    println!();
    match &outcome {
        Outcome::DryRun { command } => {
            println!("DRY RUN OK: `{command}` validated the package. Nothing was published.");
            println!("Re-run with --confirm to publish for real.");
        }
        Outcome::StubbedNoDryRun { message, would_run } => {
            println!("{message}");
            println!("Would run: `{would_run}`");
            println!("Re-run with --confirm to actually push.");
        }
        Outcome::Published { command } => {
            println!("PUBLISHED via `{command}`.");
        }
    }

    Ok(())
}

/// An isolated, detached git worktree that removes itself on request.
struct Worktree {
    repo: PathBuf,
    dir: tempfile::TempDir,
}

impl Worktree {
    fn add(repo: &Path, git_ref: &str) -> Result<Self> {
        let dir = tempfile::TempDir::new().context("creating worktree tempdir")?;
        // `worktree add` needs the target dir to not pre-exist; TempDir made it,
        // so point at a child path.
        let target = dir.path().join("wt");
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "add", "--detach"])
            .arg(&target)
            .arg(git_ref)
            .output()
            .context("spawning `git worktree add`")?;
        if !out.status.success() {
            bail!(
                "`git worktree add` failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(Worktree {
            repo: repo.to_path_buf(),
            dir,
        })
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join("wt")
    }

    fn remove(&self) -> Result<()> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["worktree", "remove", "--force"])
            .arg(self.path())
            .output()
            .context("spawning `git worktree remove`")?;
        if !out.status.success() {
            bail!(
                "`git worktree remove` failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }
}

/// Recursively copy `src` dir into `dst` (created if missing).
pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("creating dir {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("reading dir {}", src.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir(&from, &to)?;
        } else if file_type.is_symlink() {
            // Resolve and copy the target's bytes; keeps staging self-contained.
            let target = fs::read_link(&from)?;
            let resolved = if target.is_absolute() {
                target
            } else {
                from.parent().unwrap_or(src).join(target)
            };
            if resolved.is_dir() {
                copy_dir(&resolved, &to)?;
            } else {
                fs::copy(&resolved, &to).with_context(|| {
                    format!(
                        "copying symlink target {} -> {}",
                        resolved.display(),
                        to.display()
                    )
                })?;
            }
        } else {
            fs::copy(&from, &to)
                .with_context(|| format!("copying {} -> {}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// Run a native tool, streaming captured output, and surface it on failure.
pub(crate) fn run_tool(cmd: &mut Command, pretty: &str) -> Result<()> {
    println!("$ {pretty}");
    let out = cmd
        .output()
        .with_context(|| format!("spawning `{pretty}` (is the tool installed?)"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stdout.trim().is_empty() {
        print!("{stdout}");
    }
    if !stderr.trim().is_empty() {
        eprint!("{stderr}");
    }
    if !out.status.success() {
        bail!(
            "`{pretty}` failed (exit {}):\n{stdout}\n{stderr}",
            out.status.code().unwrap_or(-1)
        );
    }
    Ok(())
}
