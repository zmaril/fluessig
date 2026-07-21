# fluessig -> Django/SQLite experiment

An end-to-end demo: a schema authored in the **fluessig Rust derives** drives a
working **Django 5.2 app on SQLite**, in the primary ownership mode **(a)
`managed=False`** — fluessig owns the DDL, Django models are read/write projections.

See **[FINDINGS.md](./FINDINGS.md)** for the full runtime write-up (this is the
core deliverable). This README is just the reproduce steps.

## Layout

```
schema/                 a standalone derive crate (its own [workspace], excluded
                        from fluessig's default cargo set)
  src/lib.rs            the issue-tracker schema, authored with #[derive(Entity/Enum/Edge)]
  src/bin/emit.rs       prints catalog.json
  src/bin/dump_sql.rs   prints the fluessig-owned SQLite DDL (fluessig::sql::ddl)
  catalog.json          emitted, committed
project/                the Django project
  config/               settings + urls + wsgi (SQLite; MIGRATION_MODULES demo=None)
  demo/                 the app: generated models.py, admin.py, smoke command
  fluessig_schema.sql   the fluessig-owned SQLite DDL (generated, committed)
```

## Reproduce

Prereqs: a `fluessig-gen` release binary (`cargo build --release` at the repo root)
and Python 3.11+.

### 1. Regenerate the schema artifacts (only if you change `schema/src/lib.rs`)

```sh
cd experiments/django-sqlite/schema
cargo run --bin emit     > catalog.json                      # the catalog
cargo run --bin dump_sql > ../project/fluessig_schema.sql    # the SQLite DDL

# the Django models, from the catalog:
../../../target/release/fluessig-gen catalog.json /tmp/throwaway.rs \
  --django-models ../project/demo/models.py \
  --banner-note "Regenerate: see experiments/django-sqlite/README.md"
```

### 2. Set up the Django project (mode a: managed=False)

```sh
cd experiments/django-sqlite/project
python3 -m venv venv
./venv/bin/pip install "django>=5.2,<5.3"

# Django's own aux tables (auth/admin/sessions); the demo app is not migrated.
./venv/bin/python manage.py migrate

# apply fluessig's DDL to create the tables fluessig owns:
./venv/bin/python -c "import sqlite3; d=sqlite3.connect('db.sqlite3'); \
  d.executescript(open('fluessig_schema.sql').read()); d.commit()"
```

### 3. Run the proof

```sh
./venv/bin/python manage.py check    # -> System check identified no issues
./venv/bin/python manage.py smoke    # CRUD + enums + composite-PK-member-FK
```

### 4. Admin (optional)

```sh
DJANGO_SUPERUSER_PASSWORD=admin ./venv/bin/python manage.py createsuperuser \
  --noinput --username admin --email admin@example.com
./venv/bin/python manage.py runserver 8731
# visit http://localhost:8731/admin/  (login admin/admin)
```

Composite-PK models (Issues, Labels, IssueLabels) are **not** registerable with the
default Django admin (a Django 5.2 limitation) — see FINDINGS.md section 4.

## Notes

- The `schema/` crate is a **standalone workspace** (its own `[workspace]` table),
  so a bare `cargo build`/`test`/`clippy` at the fluessig repo root is unaffected.
- `venv/`, `__pycache__/`, and `db.sqlite3` are gitignored.
