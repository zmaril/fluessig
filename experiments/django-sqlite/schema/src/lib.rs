//! A tiny **issue tracker**, authored entirely in the fluessig Rust derive front
//! end, purpose-built to stress the interesting lowerings for the Django/SQLite
//! experiment (see `../FINDINGS.md`):
//!
//! * **to-one relations / real FKs** — `Repo → Org`, `Issue → User` (author),
//!   `Comment → Issue`, `Comment → User`.
//! * **composite key whose leading member is itself a FK** — `Issue` is keyed on
//!   `(repo, number)` where `repo: Id<Repo>` is a foreign key. This is the
//!   `gh_issues` pattern from entl, and the case that stresses Django's
//!   `CompositePrimaryKey` + `ForeignKey`-member interaction. `Label` keys the
//!   same way, on `(repo, name)`.
//! * **a composite FK reference** — `Comment.issue: Id<Issue>` expands to the two
//!   FK columns `(repo_id, issue_number)`.
//! * **enums, both shapes** — `IssueState` (name == wire value) and `IssueKind`
//!   (wire `value` distinct from the variant name, e.g. `Feature` stored as
//!   `"feat"`), to exercise the value-vs-name choice + `get_FOO_display()`.
//! * **a to-many edge** — `IssueLabel` joins `Issue ↔ Label` (its own table),
//!   with `shares(repo)` so the join carries `repo_id` once.
//! * **scalar variety** — a nullable text (`body`), a nullable datetime
//!   (`closed_at`), a bool (`is_locked`), an int with a DDL default
//!   (`comment_count = 0`), and `///`-doc'd fields (→ Django `help_text`).

use fluessig_derive::{catalog, Edge, Entity, Enum, Id};

/// Stand-in for `chrono::DateTime<Utc>` — the derive maps `DateTime<_>` to the
/// `utcDateTime` scalar (stored as RFC3339 `text` in SQLite; `DateTimeField` in
/// Django). Mirrors the entl-schema stand-in so no chrono dependency is needed.
pub struct DateTime<Tz>(core::marker::PhantomData<Tz>);
/// The UTC timezone marker for [`DateTime`].
pub struct Utc;

/// Issue priority (added in the Step-5 drift demo). Nullable on [`Issue`], so a
/// triaged issue can have no priority yet.
#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Priority {
    /// Low priority.
    Low,
    /// Medium priority.
    Medium,
    /// High priority.
    High,
}

/// Whether an issue is open or closed. `rename_all` makes the wire values
/// `OPEN` / `CLOSED` — here the stored value equals the variant name, so no
/// per-variant `value` is emitted.
#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IssueState {
    /// The issue is open.
    Open,
    /// The issue has been closed.
    Closed,
}

/// The kind of an issue. Each variant pins an explicit wire `value` that differs
/// from the variant name (`Feature` → `"feat"`), exercising the value-vs-name
/// distinction: the column stores `"feat"` while `get_kind_display()` shows
/// `Feature`.
#[derive(Enum, Clone, Copy)]
pub enum IssueKind {
    /// Something is broken.
    #[fluessig(value = "bug")]
    Bug,
    /// A new capability.
    #[fluessig(value = "feat")]
    Feature,
    /// Maintenance work.
    #[fluessig(value = "chore")]
    Chore,
}

/// An organization — the root of the ownership chain, keyed by a slug.
#[derive(Entity)]
#[fluessig(name = "orgs")]
pub struct Org {
    /// The org slug (a stable identifier), e.g. `acme`.
    #[key]
    pub id: String,
    /// The org's display name.
    pub name: String,
}

/// A person — the target of the author FKs.
#[derive(Entity)]
#[fluessig(name = "users")]
pub struct User {
    /// The user id.
    #[key]
    pub id: i64,
    /// The login handle.
    pub login: String,
    /// Display name, if the user set one.
    pub name: Option<String>,
}

/// A repository — belongs to an [`Org`] via a single-column FK, and is the
/// composite-key target that both [`Issue`] and [`Label`] key on.
#[derive(Entity)]
#[fluessig(name = "repos")]
pub struct Repo {
    /// The repo id (`org/name`).
    #[key]
    pub id: String,
    /// The owning org — a single-column to-one FK.
    pub org: Id<Org>,
    /// The short repo name.
    pub name: String,
    /// Whether the repo is private.
    pub is_private: bool,
}

/// A label — **composite-keyed** on `(repo, name)`, where `repo` is itself an
/// FK. `ref_cols` spells the `name` key part as `label_name` at referencing
/// sites (the `repo` part spells as `repo_id`).
#[derive(Entity)]
#[fluessig(name = "labels", ref_cols(name = "label_name"))]
pub struct Label {
    /// The repo the label belongs to — an FK that is also a key member.
    #[key]
    pub repo: Id<Repo>,
    /// The label text.
    #[key]
    pub name: String,
    /// A hex color, e.g. `#ff0000`.
    pub color: String,
}

/// An issue — the centerpiece. **Composite-keyed** on `(repo, number)` where
/// `repo: Id<Repo>` is a foreign key (the FK-in-composite-PK case). Carries two
/// enums, a nullable datetime, a bool, and an int with a DDL default.
#[derive(Entity)]
#[fluessig(name = "issues", ref_cols(number = "issue_number"))]
pub struct Issue {
    /// The repo the issue belongs to — an FK **and** the leading PK member.
    #[key]
    pub repo: Id<Repo>,
    /// The issue number within the repo — the second PK member.
    #[key]
    pub number: i32,
    /// The issue title.
    pub title: String,
    /// The issue body (Markdown); absent for a title-only issue.
    pub body: Option<String>,
    /// Whether the issue is open or closed.
    pub state: IssueState,
    /// What kind of issue this is (bug / feature / chore).
    pub kind: IssueKind,
    /// The issue author — a single-column to-one FK to [`User`].
    pub author: Id<User>,
    /// Whether the conversation is locked to collaborators.
    pub is_locked: bool,
    /// Number of comments — defaults to 0 at the DDL level.
    #[fluessig(default = 0)]
    pub comment_count: i32,
    /// When the issue was closed, if it has been.
    pub closed_at: Option<DateTime<Utc>>,
    /// Triage priority, once assigned (Step-5 drift demo: a nullable enum added
    /// after the schema first shipped).
    pub priority: Option<Priority>,
}

/// A comment on an issue — globally id-keyed, referencing the composite-keyed
/// [`Issue`] by a single `Id<Issue>` field that expands to `(repo_id,
/// issue_number)`, plus a to-one FK to its [`User`] author.
#[derive(Entity)]
#[fluessig(name = "comments")]
pub struct Comment {
    /// The comment id.
    #[key]
    pub id: i64,
    /// The issue this comment is on — a composite FK `(repo_id, issue_number)`.
    pub issue: Id<Issue>,
    /// The comment author.
    pub author: Id<User>,
    /// The comment body (Markdown).
    pub body: String,
    /// When the comment was posted.
    pub created_at: Option<DateTime<Utc>>,
}

/// The issue-label edge — a to-many join from [`Issue`] to [`Label`] with no
/// local key. Both the issue and the label live in the *same* repo, so the
/// label reference `#[fluessig(shares(repo))]` declares that its leading FK
/// column is the shared physical `repo_id` — the `issue_labels` table carries
/// `repo_id` once.
#[derive(Edge)]
#[fluessig(name = "issue_labels", edge(from = Issue, to = Label, expose = "labels"))]
pub struct IssueLabel {
    /// The labelled issue (source side): `(repo_id, issue_number)`.
    pub issue: Id<Issue>,
    /// The applied label (target side): `(repo_id shared, label_name)`.
    #[fluessig(shares(repo))]
    pub label: Id<Label>,
}

// The exporter half: `catalog!` collects the schema into a `fluessig_catalog`
// module whose `to_json()` prints the `catalog.json` the loader + `fluessig-gen`
// consume.
catalog! {
    name: "issue_tracker",
    version: "0.1.0",
    entities: [Org, User, Repo, Label, Issue, Comment],
    edges: [IssueLabel],
    enums: [IssueState, IssueKind, Priority],
}
