CREATE TABLE IF NOT EXISTS "comments" (
  "id" integer PRIMARY KEY,
  "repo" text NOT NULL,
  "issue_number" integer NOT NULL,
  "author" integer NOT NULL,
  "body" text NOT NULL,
  "created_at" text
);

CREATE TABLE IF NOT EXISTS "issue_labels" (
  "repo" text NOT NULL,
  "issue_number" integer NOT NULL,
  "label_name" text NOT NULL,
  PRIMARY KEY ("repo", "issue_number", "label_name")
);

CREATE TABLE IF NOT EXISTS "issues" (
  "repo" text NOT NULL,
  "number" integer NOT NULL,
  "title" text NOT NULL,
  "body" text,
  "state" text NOT NULL,
  "kind" text NOT NULL,
  "author" integer NOT NULL,
  "is_locked" integer NOT NULL,
  "comment_count" integer DEFAULT 0 NOT NULL,
  "closed_at" text,
  "priority" text,
  PRIMARY KEY ("repo", "number")
);

CREATE TABLE IF NOT EXISTS "labels" (
  "repo" text NOT NULL,
  "name" text NOT NULL,
  "color" text NOT NULL,
  PRIMARY KEY ("repo", "name")
);

CREATE TABLE IF NOT EXISTS "orgs" (
  "id" text PRIMARY KEY,
  "name" text NOT NULL
);

CREATE TABLE IF NOT EXISTS "repos" (
  "id" text PRIMARY KEY,
  "org" text NOT NULL,
  "name" text NOT NULL,
  "is_private" integer NOT NULL
);

CREATE TABLE IF NOT EXISTS "users" (
  "id" integer PRIMARY KEY,
  "login" text NOT NULL,
  "name" text
);

CREATE TABLE IF NOT EXISTS "_fluessig_meta" (
  "fingerprint" text NOT NULL,
  "format" bigint NOT NULL,
  "emitter" text,
  "compiler" text,
  "generated_at" text DEFAULT CURRENT_TIMESTAMP
);
DELETE FROM "_fluessig_meta";
INSERT INTO "_fluessig_meta" ("fingerprint", "format", "emitter", "compiler") VALUES ('395450c1de3719044b409033489fe9a967b69672216653d6e9bc4fa2b6e9953a', 1, 'fluessig-derive/0.1.0', '');
