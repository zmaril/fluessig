// straitjacket-allow-file:duplication — a full session-ledger schema is inherently
// repetitive: the union variant bodies (StateChange / AgentMessage / …) and the
// op-surface DTOs share small scalar field blocks BY DESIGN, faithfully to
// disponent.tsp. These are DISTINCT value structs with distinct fields, not a copy
// that wants a helper.
//! disponent-schema-derive — **THE DISPONENT ACID TEST**: disponent's COMPLETE
//! schema (12 entities, the `env_capabilities` edge, the tagged `EventPayload`
//! union, its observe-only / destructive MCP op hints, and the full op surface)
//! authored with the `fluessig` Rust derive front end, ADDITIVELY alongside the
//! TypeSpec `disponent.tsp` it mirrors — the acid test that de-risks disponent's
//! migration to the derive front end.
//!
//! This is the SECOND migration acid test (after `entl-schema-derive`), and it
//! surfaced the three front-end features this PR adds — features entl never
//! exercised, so entl migrated green without them:
//!
//! * **`#[derive(Union)]` + `catalog!{ unions: [...] }`** — disponent's
//!   `union EventPayload` (nine variants: state / message / toolCall / toolResult /
//!   log / usage / artifact / raw / mail). A field typed by the union
//!   (`Event.payload`) lowers to `TypeRef::Union` (twin `payload_kind` + `payload`
//!   columns) and, when an op transitively references it, `api.json`'s `unions`.
//! * **`#[fluessig(readonly)]`** — the nine observe-only ops (`environments`,
//!   `offerings`, `capabilities`, `session`, `sessions`, `workspaceLink`, `events`,
//!   `messages`, `driverPlan`) → `api.json` `"readonly": true` → the MCP
//!   `readOnlyHint`. Composes with the op kind (`events` / `driverPlan` are
//!   `@readonly @stream`).
//! * **`#[fluessig(destructive)]`** — `cancel` / `reap` → `"destructive": true` →
//!   the MCP `destructiveHint`.
//!
//! The parity test (`tests/parity.rs`) asserts the derive-emitted `catalog.json` /
//! `api.json` project to the SAME physical tables (columns + order + PK order),
//! enums, scalars, unions, and the SAME ops (with every readonly/destructive flag)
//! + models + api-unions as disponent's committed TypeSpec-emitted artifacts.

use fluessig_derive::{catalog, export, Edge, Entity, Enum, Id, Record, Scalar, Union};

// ═════════════════════════════════════════════════════════════════════════════
// Stock-type markers — zero-dep stand-ins the derive maps to built-in scalars
// (the entl-fixture convention). The derive reads *types* (tokens), never values.
// ═════════════════════════════════════════════════════════════════════════════

/// Stand-in for `chrono::DateTime<Utc>` — the derive maps `DateTime<_>` to the
/// `utcDateTime` scalar.
pub struct DateTime<Tz>(core::marker::PhantomData<Tz>);
/// The `Utc` timezone marker (only its name matters to the derive).
pub struct Utc;
/// The stock `Json` scalar (base `string`) — `Session.envHandle`, `RawObservation.data`, …
pub struct Json;
/// The stock `url` scalar (base `string`) — `Environment.endpoint`, `Session.url`,
/// `WorkspaceLink.url`. A lowercase marker so its name lowers verbatim to `url`.
#[allow(non_camel_case_types)]
pub struct url;

// ═════════════════════════════════════════════════════════════════════════════
// Scalars — disponent's minted ids + money. DispatchId/…/FanoutId refine `string`;
// `Cents` refines `int64` (which itself roots at `numeric` — the field-usage base).
// ═════════════════════════════════════════════════════════════════════════════

/// Disponent-minted identifier (UUIDv7).
#[derive(Scalar)]
#[fluessig(extends = "string")]
pub struct DispatchId(pub String);
#[derive(Scalar)]
#[fluessig(extends = "string")]
pub struct SessionUid(pub String);
/// Disponent-minted message id (UUIDv7).
#[derive(Scalar)]
#[fluessig(extends = "string")]
pub struct MessageId(pub String);
/// Disponent-minted fan-out id (UUIDv7): one broadcast, shared by its N Messages.
#[derive(Scalar)]
#[fluessig(extends = "string")]
pub struct FanoutId(pub String);
/// Money in integer cents, USD.
#[derive(Scalar)]
#[fluessig(extends = "int64")]
pub struct Cents(pub i64);

// ═════════════════════════════════════════════════════════════════════════════
// Enums — wire values are the (snake_case) stored member names.
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "snake_case")]
pub enum EnvKind {
    Local,
    ExeDev,
    Modal,
    ClaudeCodeWeb,
    Custom,
}

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "snake_case")]
pub enum SessionState {
    Queued,
    Provisioning,
    Running,
    NeedsInput,
    Completed,
    Failed,
    Cancelled,
    Lost,
}

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "snake_case")]
pub enum ExitReason {
    Ok,
    Error,
    Signal,
    Timeout,
    Budget,
    Setup,
    Unknown,
}

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "snake_case")]
pub enum IsolationKind {
    None,
    Worktree,
    Container,
    Vm,
}

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "snake_case")]
pub enum TemplateKind {
    VmImage,
    ContainerImage,
}

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "snake_case")]
pub enum CapabilityKind {
    Dispatch,
    Interact,
    ObserveStream,
    ObservePoll,
    ListSessions,
    Resume,
    Cancel,
    Teardown,
    IsolationWorktree,
    IsolationContainer,
    IsolationVm,
    Templates,
    ArtifactFetch,
    UsageReport,
}

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "snake_case")]
pub enum EventKind {
    State,
    Message,
    ToolCall,
    ToolResult,
    Log,
    Usage,
    Artifact,
    Raw,
    Mail,
}

/// The three principals a message can move between (manager↔worker comms).
#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "snake_case")]
pub enum Party {
    Manager,
    Worker,
    User,
}

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "snake_case")]
pub enum Fidelity {
    Exact,
    Derived,
    Scraped,
}

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "snake_case")]
pub enum ArtifactKind {
    Branch,
    PullRequest,
    Patch,
    File,
    Report,
    Url,
}

#[derive(Enum, Clone, Copy)]
#[fluessig(rename_all = "snake_case")]
pub enum McpRole {
    Supervisor,
    Worker,
}

// ═════════════════════════════════════════════════════════════════════════════
// The tagged union — the whole point of this acid test (feature A).
// ═════════════════════════════════════════════════════════════════════════════

/// One observation's body — the union's variant tag is the wire discriminator.
#[derive(Union)]
pub enum EventPayload {
    State(StateChange),
    Message(AgentMessage),
    ToolCall(ToolCallInfo),
    ToolResult(ToolResultInfo),
    Log(LogLine),
    Usage(UsageDelta),
    Artifact(ArtifactRef),
    Raw(RawObservation),
    Mail(MailRef),
}

// ── union variant bodies (value structs) ──

#[derive(Record)]
pub struct StateChange {
    pub from: SessionState,
    pub to: SessionState,
}
#[derive(Record)]
pub struct AgentMessage {
    pub role: String,
    pub text: String,
}
#[derive(Record)]
pub struct ToolCallInfo {
    pub tool: String,
    pub input: Option<Json>,
}
#[derive(Record)]
pub struct ToolResultInfo {
    pub tool: String,
    pub ok: bool,
    pub output: Option<String>,
}
#[derive(Record)]
pub struct LogLine {
    pub line: String,
}
#[derive(Record)]
pub struct UsageDelta {
    pub model_id: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cost_cents: Option<Cents>,
}
#[derive(Record)]
pub struct ArtifactRef {
    pub artifact_idx: i64,
}
/// Pointer + the fields a reader needs to triage a `mail` event without
/// fetching the Message: direction (sender/recipient), the fan-out it belongs
/// to, and its topic (so a reader can group by topic for latest-wins).
#[derive(Record)]
pub struct MailRef {
    pub message_id: MessageId,
    pub sender: Party,
    pub recipient: Party,
    pub fanout_id: FanoutId,
    pub topic: Option<String>,
}
#[derive(Record)]
pub struct RawObservation {
    pub source: String,
    pub data: Json,
}

// ═════════════════════════════════════════════════════════════════════════════
// The environment side
// ═════════════════════════════════════════════════════════════════════════════

/// Somewhere work can run. Config supplies these; the shipped catalog fills offerings + capabilities.
#[derive(Entity)]
#[fluessig(name = "environments")]
pub struct Environment {
    #[key]
    pub slug: String,
    pub kind: EnvKind,
    pub display_name: Option<String>,
    /// Address only — never a credential (secrets live in config/templates).
    pub endpoint: Option<url>,
    pub last_probed_at: Option<DateTime<Utc>>,
}

/// One capability row (the edge target of Environment.capabilities).
#[derive(Entity)]
#[fluessig(name = "capabilities")]
pub struct Capability {
    #[key]
    pub capability: CapabilityKind,
}

/// The `env_capabilities` edge (feature-adjacent): source Environment (key `slug`),
/// target Capability (enum key `capability`), plus the `detail` edge property. The
/// edge struct's NAME is the `relationProperties` name (`CapabilityDetail`).
#[derive(Edge)]
#[fluessig(name = "env_capabilities", edge(from = Environment, to = Capability, expose = "capabilities"))]
pub struct CapabilityDetail {
    pub slug: Id<Environment>,
    pub capability: Id<Capability>,
    /// Env-specific texture: poll granularity, supported template kinds, …
    pub detail: Option<Json>,
}

/// A reusable starting state, curated out-of-band (auth baked in by hand).
#[derive(Entity)]
#[fluessig(name = "templates")]
pub struct Template {
    #[key]
    pub name: String,
    pub kind: TemplateKind,
    pub locator: String,
    pub setup: Option<String>,
    pub note: Option<String>,
}

/// A coding agent program (claude-code, codex, …).
#[derive(Entity)]
#[fluessig(name = "agents")]
pub struct Agent {
    #[key]
    pub name: String,
    pub version: Option<String>,
}

/// A model an agent can run with.
#[derive(Entity)]
#[fluessig(name = "models")]
pub struct AgentModel {
    #[key]
    pub id: String,
    pub provider: Option<String>,
    pub family: Option<String>,
}

/// env × agent × model availability (from the shipped catalog + config).
#[derive(Entity)]
#[fluessig(name = "offerings")]
pub struct Offering {
    #[key]
    pub env_slug: Id<Environment>,
    #[key]
    pub agent_name: Id<Agent>,
    #[key]
    pub model_id: Id<AgentModel>,
    pub is_default: bool,
}

// ═════════════════════════════════════════════════════════════════════════════
// The work side
// ═════════════════════════════════════════════════════════════════════════════

/// The immutable request. Never mutated after dispatch(); lifecycle lives on sessions.
#[derive(Entity)]
#[fluessig(name = "dispatches")]
pub struct Dispatch {
    #[key]
    pub id: DispatchId,
    pub created_at: DateTime<Utc>,
    pub title: Option<String>,
    /// The brief — the whole task spec, free-form. Structure belongs to consumers.
    pub brief: String,
    /// Workspace: URL or local path; empty = no repo (pure-prompt work).
    pub repo: Option<String>,
    pub git_ref: Option<String>,
    pub isolation: IsolationKind,
    pub fetch_remote: Option<bool>,
    pub template_name: Option<Id<Template>>,
    pub setup: Option<String>,
    pub env_slug: Id<Environment>,
    pub agent_name: Id<Agent>,
    pub model_id: Option<Id<AgentModel>>,
    pub timeout_secs: Option<i32>,
    pub max_budget: Option<Cents>,
    /// MCP recursion depth: 0 = dispatched by the host program.
    pub via_mcp_depth: i32,
    /// Selection tags — the PRIMARY handle a message fan-out addresses.
    pub tags: Option<Vec<String>>,
    /// Consumer labels, opaque to disponent.
    pub labels: Option<Json>,
}

/// One attempt at a dispatch, mirroring one env-side resource.
#[derive(Entity)]
#[fluessig(name = "sessions")]
pub struct Session {
    #[key]
    pub uid: SessionUid,
    pub dispatch_id: Id<Dispatch>,
    pub state: SessionState,
    /// The env's own handle(s): tmux session name, VM name, web session id/url.
    pub env_handle: Option<Json>,
    /// Human-facing view URL when the env has one (ttyd, web session page).
    pub url: Option<url>,
    pub resumed_from: Option<Id<Session>>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub exit_reason: Option<ExitReason>,
    pub exit_detail: Option<String>,
    /// Set by reap(): resources torn down, row archived. Null = still on the board.
    pub reaped_at: Option<DateTime<Utc>>,
}

/// One observation on a session's timeline (containment rides the key: the
/// session FK + the position, like Artifact/Usage — to-manys are queries).
#[derive(Entity)]
#[fluessig(name = "events")]
pub struct Event {
    #[key]
    pub session_uid: Id<Session>,
    #[key]
    pub idx: i64,
    pub ts: DateTime<Utc>,
    pub kind: EventKind,
    pub fidelity: Fidelity,
    pub payload: EventPayload,
}

/// Something the session produced.
#[derive(Entity)]
#[fluessig(name = "artifacts")]
pub struct Artifact {
    #[key]
    pub session_uid: Id<Session>,
    #[key]
    pub idx: i64,
    pub kind: ArtifactKind,
    /// Branch name, PR URL, path, … — a pointer, not the bytes.
    pub locator: String,
    pub meta: Option<Json>,
}

/// Best-effort accounting, per session per model.
#[derive(Entity)]
#[fluessig(name = "usage")]
pub struct Usage {
    #[key]
    pub session_uid: Id<Session>,
    #[key]
    pub model_id: Id<AgentModel>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_cents: Cents,
}

/// One message dropped in one inbox — the manager↔worker communication primitive.
#[derive(Entity)]
#[fluessig(name = "messages")]
pub struct Message {
    #[key]
    pub id: MessageId,
    pub created_at: DateTime<Utc>,
    pub sender: Party,
    pub recipient: Party,
    /// The worker session this message rides.
    pub session_uid: Id<Session>,
    /// The payload — free-form text, like the brief.
    pub body: String,
    /// Threading: the message this one replies to. Null for an unsolicited directive.
    pub in_reply_to: Option<Id<Message>>,
    /// One logical Manager broadcast → N Messages that all share this id.
    pub fanout_id: FanoutId,
    /// Supersession key (latest-wins per (recipient, topic)). Null = standalone.
    pub topic: Option<String>,
    /// Stamped by the recipient's `ack`. Null = delivered but not yet acknowledged.
    pub acked_at: Option<DateTime<Utc>>,
}

// ═════════════════════════════════════════════════════════════════════════════
// DTOs — op-surface value structs
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Record)]
pub struct OpenOptions {
    pub config_path: Option<String>,
    /// Default: a managed local SQLite file. "none" = memory-only; any driver-plan DSN otherwise.
    pub sink: Option<String>,
}
#[derive(Record)]
pub struct DispatchSpec {
    pub brief: String,
    pub env: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub title: Option<String>,
    pub repo: Option<String>,
    pub git_ref: Option<String>,
    pub isolation: Option<IsolationKind>,
    pub fetch_remote: Option<bool>,
    pub template: Option<String>,
    pub setup: Option<String>,
    pub timeout_secs: Option<i32>,
    pub max_budget: Option<Cents>,
    /// Skip catalog validation of (env, agent, model).
    pub unchecked: Option<bool>,
    pub tags: Option<Vec<String>>,
    pub labels: Option<Json>,
}
#[derive(Record)]
pub struct SessionFilter {
    pub env: Option<String>,
    pub state: Option<SessionState>,
    pub dispatch_id: Option<DispatchId>,
}
/// An editor link into a session's working directory.
#[derive(Record)]
pub struct WorkspaceLink {
    pub session_uid: SessionUid,
    pub available: bool,
    pub url: Option<url>,
    pub detail: Option<String>,
}
#[derive(Record)]
pub struct EventOptions {
    pub session_uid: Option<SessionUid>,
    pub after_idx: Option<i64>,
    pub kinds: Option<Vec<EventKind>>,
}
/// Where a Manager-sent message goes.
#[derive(Record)]
pub struct SendTarget {
    pub tags: Option<Vec<String>>,
    pub sessions: Option<Vec<SessionUid>>,
    pub user: Option<SessionUid>,
}
/// Filter for the `messages` read. Absent fields don't constrain.
#[derive(Record)]
pub struct MessagesFilter {
    pub fanout_id: Option<FanoutId>,
    pub recipient: Option<Party>,
    pub session_uid: Option<SessionUid>,
    pub topic: Option<String>,
    pub latest_per_topic: Option<bool>,
}
#[derive(Record)]
pub struct ReconcileReport {
    pub adopted: i32,
    pub confirmed: i32,
    pub lost: i32,
    pub torn_down: i32,
}
#[derive(Record)]
pub struct McpOptions {
    pub transport: Option<String>,
    pub role: Option<McpRole>,
    pub max_depth: Option<i32>,
}
#[derive(Record)]
pub struct DriverPlanOptions {
    pub dialect: Option<String>,
    pub tables: Option<Vec<String>>,
}
#[derive(Record)]
pub struct Statement {
    pub sql: String,
    pub params: Json,
}
/// What an environment can do: one row per (env, capability) the catalog
/// advertises. Mirrors the env_capabilities edge as a flat, returnable value
/// struct (the closed CapabilityKind vocabulary, plus open detail).
#[derive(Record)]
pub struct EnvCapability {
    pub env_slug: String,
    pub capability: CapabilityKind,
    /// Env-specific texture (poll granularity, supported template kinds, …), when known.
    pub detail: Option<Json>,
}

// ═════════════════════════════════════════════════════════════════════════════
// The op surface — features B (@readonly) + C (@destructive), composing with kind
// ═════════════════════════════════════════════════════════════════════════════

/// An open disponent instance. A unit-ish struct keeps the op root a *type*.
pub struct Disponent {
    _private: (),
}

#[export]
impl Disponent {
    #[fluessig(ctor)]
    pub fn open(options: Option<OpenOptions>) -> Self {
        let _ = options;
        Disponent { _private: () }
    }

    #[fluessig(readonly)]
    pub fn environments(&self) -> Vec<Environment> {
        Vec::new()
    }

    pub fn refresh(&self, env_slug: Option<String>) -> Vec<Environment> {
        let _ = env_slug;
        Vec::new()
    }

    /// The offerings table: every env × agent × model the catalog knows.
    #[fluessig(readonly)]
    pub fn offerings(&self) -> Vec<Offering> {
        Vec::new()
    }

    /// Per-env capabilities: what each environment can do, one row per (env, capability).
    #[fluessig(readonly)]
    pub fn capabilities(&self) -> Vec<EnvCapability> {
        Vec::new()
    }

    pub fn dispatch(&self, spec: DispatchSpec) -> Session {
        let _ = spec;
        unimplemented!()
    }

    #[fluessig(readonly)]
    pub fn session(&self, uid: SessionUid) -> Option<Session> {
        let _ = uid;
        None
    }

    #[fluessig(readonly)]
    pub fn sessions(&self, filter: Option<SessionFilter>) -> Vec<Session> {
        let _ = filter;
        Vec::new()
    }

    /// Return an editor link (VS Code deep link) into the session's working directory.
    #[fluessig(readonly)]
    pub fn workspace_link(&self, session_uid: SessionUid) -> WorkspaceLink {
        let _ = session_uid;
        unimplemented!()
    }

    #[fluessig(readonly, stream)]
    pub fn events(&self, options: Option<EventOptions>) -> impl Iterator<Item = Event> {
        let _ = options;
        std::iter::empty()
    }

    /// The one messaging primitive: a Manager `to` names a tagged worker subset or the user.
    pub fn send(
        &self,
        body: String,
        to: Option<SendTarget>,
        in_reply_to: Option<MessageId>,
        topic: Option<String>,
    ) -> Vec<Message> {
        let _ = (body, to, in_reply_to, topic);
        Vec::new()
    }

    /// Acknowledge a message you received: stamps `ackedAt`. Idempotent.
    pub fn ack(&self, message_id: MessageId) {
        let _ = message_id;
    }

    /// Read Messages, filtered.
    #[fluessig(readonly)]
    pub fn messages(&self, filter: Option<MessagesFilter>) -> Vec<Message> {
        let _ = filter;
        Vec::new()
    }

    #[fluessig(destructive)]
    pub fn cancel(&self, session_uid: SessionUid) -> Session {
        let _ = session_uid;
        unimplemented!()
    }

    pub fn resume(&self, session_uid: SessionUid) -> Session {
        let _ = session_uid;
        unimplemented!()
    }

    #[fluessig(destructive)]
    pub fn reap(&self, session_uid: SessionUid) -> Session {
        let _ = session_uid;
        unimplemented!()
    }

    pub fn reconcile(&self) -> ReconcileReport {
        unimplemented!()
    }

    #[fluessig(readonly, stream)]
    pub fn driver_plan(
        &self,
        options: Option<DriverPlanOptions>,
    ) -> impl Iterator<Item = Statement> {
        let _ = options;
        std::iter::empty()
    }

    /// Blocking wait — hand-written per binding (event-loop/GVL specifics).
    #[fluessig(manual)]
    pub fn wait(&self, session_uid: SessionUid, timeout_secs: i32) -> Session {
        let _ = (session_uid, timeout_secs);
        unimplemented!()
    }

    /// Long-running MCP server over this instance — hand-written per binding.
    #[fluessig(manual)]
    pub fn serve_mcp(&self, options: Option<McpOptions>) {
        let _ = options;
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// The exporter — replaces `(cd emitter && node emit.mjs ../schema/disponent.tsp)`.
// ═════════════════════════════════════════════════════════════════════════════

catalog! {
    name: "disponent.tsp",
    version: "0",
    entities: [
        Environment, Capability, Template, Agent, AgentModel, Offering,
        Dispatch, Session, Event, Artifact, Usage, Message,
    ],
    edges: [CapabilityDetail],
    records: [
        // union variant bodies
        StateChange, AgentMessage, ToolCallInfo, ToolResultInfo, LogLine,
        UsageDelta, ArtifactRef, RawObservation, MailRef,
        // op-surface DTOs
        OpenOptions, DispatchSpec, SessionFilter, WorkspaceLink, EventOptions,
        SendTarget, MessagesFilter, ReconcileReport, McpOptions, DriverPlanOptions,
        Statement, EnvCapability,
    ],
    enums: [
        EnvKind, SessionState, ExitReason, IsolationKind, TemplateKind,
        CapabilityKind, EventKind, Party, Fidelity, ArtifactKind, McpRole,
    ],
    unions: [EventPayload],
    scalars: [DispatchId, SessionUid, MessageId, FanoutId, Cents],
    api: [Disponent],
}
