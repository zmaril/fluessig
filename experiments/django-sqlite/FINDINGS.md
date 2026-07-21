# fluessig → Django/SQLite: runtime findings

<!-- straitjacket-allow-file:duplication — the pasted runtime evidence restates
     code-shaped patterns (FK-join traces, enum round-trips) by design; the
     repetition is the proof, not a refactor target. -->

**What this is.** A real, running proof that a **fluessig-authored schema drives a
working Django app on SQLite**. A bespoke issue-tracker schema is authored in the
fluessig Rust derives (`schema/src/lib.rs`), emitted to `catalog.json`, and from
that one catalog we generate **both** the Django models (`--django-models`) **and**
the SQLite DDL (`fluessig::sql::ddl`, `Dialect::Sqlite`). Django then runs CRUD,
enums, admin, and migrations against tables fluessig owns.

The primary mode demonstrated is **(a) `managed=False`**: fluessig owns the SQLite
DDL; the Django models are read/write projections over fluessig-owned tables. We
also gather concrete evidence of what **mode (b)** (Django `makemigrations` owns the
DDL) would cost.

Everything here is runtime evidence — every block below is pasted real output.

## Environment

- Python 3.11.15, **Django 5.2.16** (5.2+ required for `CompositePrimaryKey`).
- SQLite (`db.sqlite3`), stdlib `sqlite3` to apply fluessig's DDL.
- The demo schema is a **bespoke** derive crate authored for this experiment
  (`schema/`), not a reused catalog — it deliberately exercises FK-in-composite-PK,
  two enum shapes, a to-many edge, and scalar variety.

## The demo schema (what it exercises)

`schema/src/lib.rs`, 6 entities + 1 edge + 3 enums:

| entity | shape exercised |
| --- | --- |
| `Org` | scalar PK (`id: String`) |
| `User` | scalar PK (`id: i64`), a nullable text (`name`) |
| `Repo` | single-column **FK** `org: Id<Org>`, a bool |
| `Label` | **composite PK `(repo, name)`** where `repo` is an FK |
| `Issue` | **composite PK `(repo, number)`** where `repo` is an FK; two enums; nullable datetime; bool; int with **DDL default** |
| `Comment` | scalar PK; a **composite FK** `issue: Id<Issue>` -> `(repo, issue_number)`; FK author |
| `IssueLabel` (edge) | **to-many** join `Issue <-> Label`, `shares(repo)` |

Enums: `IssueState` (name == wire value: `OPEN`/`CLOSED`), `IssueKind`
(**wire value != name**: `Feature` stored as `"feat"`), `Priority` (added in the
Step-5 drift demo).

---

## 1. What mapped cleanly

**Single-column FKs -> real `ForeignKey`.** `Repo.org`, `Issue.author`,
`Comment.author` each become a `ForeignKey` with an explicit `db_column`,
`on_delete=DO_NOTHING`, and `related_name`. Cross-relation queries work:

```
   Issue (repo,number)=(acme/widgets,1)
     .repo.name          = widgets
     .repo.org.name      = Acme Corp   (2-hop FK join)
     .author.login       = alice
   filter(author=alice)  -> [1]
   comments on issue     -> [(100, 'bob')]
```

`select_related("repo", "repo__org", "author")` resolves a two-hop FK join over
managed=False tables — the ORM is fully live, not read-only-in-name.

**Enums -> `CharField(choices=...)`, value-vs-name correct.** The stored value is
the wire value; `get_FOO_display()` returns the variant name. Round-trip proof:

```
   state: stored='OPEN'  display='OPEN'
   kind : stored='feat'  display='Feature'   (value 'feat' != name 'Feature')
   asserts passed: wire value stored, display name resolved via choices
```

The emitter sizes `max_length` from the longest wire value and emits
`choices=[("feat", "Feature"), ...]` — so `Feature` (name) is the label and `feat`
(value) is what lands in the column, exactly as fluessig's catalog says.

**Composite keys -> `CompositePrimaryKey`.** `Issue`, `Label`, and the
`IssueLabel` edge all key on multiple columns; each emits
`pk = models.CompositePrimaryKey(...)`. Querying by the PK tuple works:
`Issues.objects.get(pk=('acme/widgets', 1))`.

**DDL defaults, nullability, docs.** `comment_count` carries `DEFAULT 0` in the
fluessig DDL; nullable fields (`body`, `closed_at`, `User.name`) emit
`null=True, blank=True`; every `///` doc comment flows through to Django
`help_text=` (visible in the admin form screenshot) and to class docstrings.

**Composite FK -> honest scalar fallback.** `Comment.issue` is a two-column FK
`(repo, issue_number)`. Django's `ForeignKey` is single-column, so the emitter
keeps it as scalar columns with a note — and says so:

```python
class Comments(models.Model):
    ...
    # NOTE: multi-column FK to Issue — Django ForeignKey is single-column; kept as scalar column(s).
    repo = models.TextField(help_text="The issue this comment is on — a composite FK `(repo_id, issue_number)`.")
    issue_number = models.IntegerField(...)
    author = models.ForeignKey("Users", ...)   # single-column FK -> real ForeignKey
```

Queries against the scalar columns work normally
(`Comments.objects.filter(repo=repo.id, issue_number=1)`).

---

## 2. The decisive test — ForeignKey as a CompositePrimaryKey member

This is the case that stresses Django's `CompositePrimaryKey` + `ForeignKey`
interaction, mirroring entl's real `gh_issues` (keyed `(repo, number)` where `repo`
is an FK). The emitter produces:

```python
class Issues(models.Model):
    repo = models.ForeignKey("Repos", on_delete=models.DO_NOTHING, db_column="repo",
                             related_name="issues_repo", help_text="...")
    number = models.IntegerField(help_text="...")
    ...
    # Composite key (requires Django 5.2+ CompositePrimaryKey).
    pk = models.CompositePrimaryKey("repo", "number")
```

i.e. `CompositePrimaryKey("repo", "number")` names a **`ForeignKey`** field (`repo`)
as a PK member.

### Verdict: **Django ACCEPTS it.** No emitter change was needed.

Decisive runtime output (`manage.py smoke`, section 5):

```
5. COMPOSITE-PK-MEMBER-FK — the decisive test
   Issues._meta.pk               = <django.db.models.fields.composite.CompositePrimaryKey: pk>
   Issues.pk field type          = CompositePrimaryKey
   Issues 'repo' member field    = ForeignKey (is_relation=True)
   .pk of the created issue      = ('acme/widgets', 1)
   Issues.objects.get(pk=('acme/widgets', 1)) -> title='Widget explodes on Tuesdays'
   RESULT: Django accepts a ForeignKey as a CompositePrimaryKey member — import, migrate-skip, create, and query all work.
```

And `manage.py check` reports **no issues** — the model imports, passes system
checks, and the `pk` tuple round-trips (`.pk == ('acme/widgets', 1)`). Creating the
row through the ORM with `repo=<Repo instance>` works, and so does querying by the
composite `pk`. `Label` (also FK-in-composite-PK) behaves identically.

So the emitter's decision to spell the PK member by the **relation field name**
(`repo`, not `repo_id`) is correct: Django resolves the `CompositePrimaryKey`
member against the `ForeignKey`'s attname and is happy.

---

## 3. CRUD + enum smoke — full output

`python manage.py smoke` (source: `demo/management/commands/smoke.py`):

```
======================================================================
1. CREATE across relations: Org -> Repo -> User -> Issue -> Comment
======================================================================
   created org='acme' repo='acme/widgets' users='alice','bob'
   created Issue pk=(repo='acme/widgets', number=1) via ForeignKey member — SUCCESS
   created Comment id=100 on issue (repo=acme/widgets, number=1)
   created Label pk=(repo='acme/widgets', name='bug') + issue_labels edge row

======================================================================
2. QUERY across FK relations (select_related + filter)
======================================================================
   Issue (repo,number)=(acme/widgets,1)
     .repo.name          = widgets
     .repo.org.name      = Acme Corp   (2-hop FK join)
     .author.login       = alice
   filter(author=alice)  -> [1]
   comments on issue     -> [(100, 'bob')]
   labels on issue (edge)-> ['bug']

======================================================================
3. ENUM round-trip (stored wire value vs display name)
======================================================================
   state: stored='OPEN'  display='OPEN'
   kind : stored='feat'  display='Feature'   (value 'feat' != name 'Feature')
   asserts passed: wire value stored, display name resolved via choices

======================================================================
4. UPDATE a field + read back (close the issue)
======================================================================
   after update: state='CLOSED' display='CLOSED' closed_at=datetime.datetime(2026, 7, 21, 9, 30, tzinfo=datetime.timezone.utc) comment_count=1
   asserts passed: update() over managed=False table persisted

======================================================================
5. COMPOSITE-PK-MEMBER-FK — the decisive test
======================================================================
   ... (see section 2 above) ...

SMOKE OK — all CRUD/enum/composite-PK assertions passed.
```

---

## 4. Admin site (the free-win check)

`python manage.py check` -> **`System check identified no issues (0 silenced).`**

**Registration:** we register generically over the app's model registry
(`demo/admin.py`). Result:

- **Registered (single-PK):** `Comments`, `Orgs`, `Repos`, `Users`.
- **REJECTED (composite-PK):** `Issues`, `Labels`, `IssueLabels`, each with:

  ```
  django.core.exceptions.ImproperlyConfigured: The model IssueLabels has a
  composite primary key, so it cannot be registered with admin.
  ```

This is a **known Django limitation as of 5.2**: `ModelAdmin` refuses any model
with a `CompositePrimaryKey`. So exactly the models the schema most wants an
editor for (issues, labels) are admin-invisible under the default admin. We surface
the skip on `demo.admin.UNREGISTERABLE_COMPOSITE_PK` rather than swallow it.

**Screenshots** (headless Chromium against the dev server), saved to the session
scratchpad:

- `admin_index.png` — the admin index showing the DEMO app with `Comments`, `Orgs`,
  `Repos`, `Users` (composite-PK models correctly absent).
- `admin_changelist.png` — the `repos` changelist showing the real row
  `Repos object (acme/widgets)` read live from the fluessig-owned table.
- `admin_form.png` — the `comments` change form: `help_text` from the fluessig doc
  comments, the composite FK as scalar `Repo`/`Issue number` fields, the `Author`
  FK dropdown, and the `Created at` datetime widget — all driven by the generated
  model.

---

## 5. The drift story (both modes, concrete)

### Mode (a) — `managed=False` (the shipped default)

`migrate` creates only Django's own aux tables; the demo tables come from applying
fluessig's DDL (`sqlite3 db.sqlite3 < fluessig_schema.sql`). The `demo` app is not
Django-migrated at all — `settings.MIGRATION_MODULES = {"demo": None}`, the
idiomatic companion to per-model `managed=False`.

We then **changed the schema** in the derive crate (added a nullable `priority`
enum to `Issue`), regenerated `catalog.json`, `models.py`, and
`fluessig_schema.sql`, and asked Django to diff:

```
$ python manage.py makemigrations
No changes detected
```

**Django's autodetector is bypassed entirely** — a real schema change produced *no*
migration. Reconciling the database is a fluessig concern (here, an `ALTER TABLE`;
in the entl/disponent model, the fingerprint-driven drop-and-rebuild cache):

```
issues columns now: ['repo','number','title','body','state','kind','author',
                     'is_locked','comment_count','closed_at','priority']
priority stored= 'HIGH'  display= 'HIGH'   # new field works end-to-end afterward
```

**Two honest nuances found:**

1. `MIGRATION_MODULES={"demo": None}` is what yields the clean *"No changes
   detected"*. Targeting the disabled app explicitly (`makemigrations demo`) instead
   raises `ValueError: Django can't create migrations for app 'demo' because
   migrations have been disabled via the MIGRATION_MODULES setting.` — so the
   demonstration uses the all-apps form.
2. With `managed=False` **alone** (migrations *not* disabled), `makemigrations`
   does **not** stay silent — it writes a no-op `CreateModel`/`AddField` migration
   carrying `options={'managed': False}` (state-only, zero DDL). So "the models are
   `managed=False`, therefore no migration" is not quite the mechanism;
   `managed=False` suppresses the *DDL*, and disabling the app's migration module
   suppresses the *state diff*. Both are needed for the clean mode-(a) story.

### Mode (b) — Django owns the DDL (the cost)

To show what mode (b) buys, we flipped **one** model (`Orgs`) to `managed=True` in a
throwaway edit, re-enabled demo migrations, and ran `makemigrations --dry-run`.
Django **does** autodiff real DDL:

```python
migrations.CreateModel(
    name='Orgs',
    fields=[
        ('id', models.TextField(help_text='The org slug (a stable identifier), e.g. `acme`.', primary_key=True, serialize=False)),
        ('name', models.TextField(help_text="The org's display name.")),
    ],
    options={
        'db_table': 'orgs',
        'managed': True,        # <-- a REAL table Django would CREATE
    },
),
```

So mode (b) gives you Django's migration autodetector: real `CreateModel`/
`AddField`/`AlterField` diffs, `migrate` runs the DDL, and every composite-PK model
deconstructs cleanly into a migration (the `CompositePrimaryKey(...)` survives the
round-trip). We reverted the edit afterward.

**A nuance worth flagging for mode (b):** in the generated migration, Django
**omits the `ForeignKey` fields whose target is `managed=False`** — e.g. `Comments`
in the migration has `id/repo/issue_number/body/created_at` but *not* the `author`
FK. So a mixed managed/unmanaged app produces migrations that don't fully describe
the FK graph; going to mode (b) is really an all-or-nothing flip.

---

## 6. What Django fought on (summary)

| friction | severity | detail |
| --- | --- | --- |
| **Composite-PK models in admin** | **blocking (for admin)** | `ModelAdmin` refuses `CompositePrimaryKey` models outright (Django 5.2). Issues/Labels get no admin UI. |
| Multi-column FK | expected | Django `ForeignKey` is single-column; the composite FK (`Comment.issue`) falls back to scalar columns. Correct, but you lose the relation accessor. |
| makemigrations semantics | papercut | `managed=False` alone still emits no-op state migrations; you also need `MIGRATION_MODULES={app: None}` for a truly quiet app, and then can't target it explicitly. |
| FK omission in mode (b) | papercut | FKs to `managed=False` targets are dropped from generated migrations. |
| DecimalField precision | not exercised | fluessig carries no numeric precision; the emitter defaults to `max_digits=38, decimal_places=9`. The demo schema has no decimal field, so this was **not** exercised at runtime — flagged from reading the emitter, not observed. |

---

## 7. The ownership tension (a vs b) and recommendation

**Mode (a), `managed=False` — recommended default.** It *works*: full CRUD,
cross-FK queries, enum display, composite PKs (including FK-in-PK), and a partial
admin (single-PK models only). It mirrors fluessig's own philosophy — the sink owns
the schema, the ORM is a projection — exactly as the SQLAlchemy read-plane does with
its `before_create`/`before_drop` guards. The cost is that **Django's migration
autodetector is bypassed**: schema evolution is fluessig's job (regenerate DDL +
reconcile), not `makemigrations`.

**Mode (b), `managed=True`** would hand Django the migration autodetector — real,
reviewable DDL diffs and `migrate`-driven evolution. But it makes **Django the
schema owner**, creating *two* sources of truth (the fluessig catalog and Django's
migration history) that can silently diverge. That directly contradicts fluessig's
"impl is the interface" — the schema would live in two places, and only one is
authoritative.

**Recommendation: ship mode (a) as the emitter's contract** (`managed=False` +
document `MIGRATION_MODULES={app: None}`), and treat mode (b) as a documented,
opt-in escape hatch for teams that want Django-owned migrations and are willing to
give up single-sourcing. This keeps the emitter honest with the rest of fluessig:
the catalog is the source of truth, Django reads it.

## 8. Verdict — first-class emitter, or stay an experiment?

**Worth a first-class `--django-models` emitter, with two caveats to settle first.**
The mapping is genuinely good — FKs, composite PKs (incl. FK-in-PK), enums with
value-vs-name, defaults, nullability, and docs->help_text all land correctly and
were proven at runtime. The honest fallbacks (multi-column FK -> scalar + note) are
the right call.

What we'd want **Zack** to decide before productizing:

1. **Composite-PK admin.** The single biggest friction. Options: (a) accept the gap
   and document it; (b) emit a custom `ModelAdmin`/proxy that Django admin tolerates;
   (c) wait for upstream Django to lift the restriction. This determines whether
   "free Django admin" is a real selling point or a partial one.
2. **Where the emitter lives.** Django is a *consumer* target like the SQLAlchemy
   read-plane, not a bindgen backend — so a `--django-models` flag on `fluessig-gen`
   (already implemented) is the right shape. Confirm the emitter should also emit the
   `MIGRATION_MODULES` guidance / a ready-made `apps.py`, so mode (a) is turnkey.
3. **Mode (b) support.** Decide whether to offer a `managed=True` variant at all, or
   hold the line on single-sourcing and ship mode (a) only.

Net: the runtime evidence says this works and is useful; the open questions are
product/ergonomics, not feasibility.
