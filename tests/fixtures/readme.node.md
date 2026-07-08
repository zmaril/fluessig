# entl — Node/Bun quickstart

`entl` describes a typed entity graph once and projects it
everywhere. This guide shows the Node/Bun surface, generated from one template.

## Install

```sh
bun add entl
```

## Quickstart

Open the engine, run one query, and print the rows. Save this as
`quickstart.ts`.

```typescript
import { Engine } from "entl";

const engine = Engine.open("data.duckdb");
for (const row of await engine.query("SELECT * FROM commits LIMIT 5")) {
  console.log(row);
}
```

Run it with the Node/Bun toolchain and you should see five rows.

> Note: in Node/Bun the query runs off-thread, so nothing blocks the caller.

## Learn more

See the full reference for the rest of the `entl` op surface.

<!-- straitjacket-allow-file:duplication — parallel per-language renders of one template -->
