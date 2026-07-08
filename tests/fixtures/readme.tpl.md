# {{ catalog.name }} — {{ lang }} quickstart

`{{ catalog.name }}` describes a typed entity graph once and projects it
everywhere. This guide shows the {{ lang }} surface, generated from one template.

## Install

```sh
{{ lang.install }}
```

## Quickstart

<!-- fl:each -->
Open the engine, run one query, and print the rows. Save this as
`quickstart.{{ lang.ext }}`.

<!-- fl:lang rust -->
```rust
use {{ pkg }}::Engine;

fn main() -> anyhow::Result<()> {
    let engine = Engine::open("data.duckdb")?;
    for row in engine.query("SELECT * FROM commits LIMIT 5")? {
        println!("{row:?}");
    }
    Ok(())
}
```
<!-- fl:lang node -->
```typescript
import { Engine } from "{{ pkg }}";

const engine = Engine.open("data.duckdb");
for (const row of await engine.query("SELECT * FROM commits LIMIT 5")) {
  console.log(row);
}
```
<!-- fl:lang python -->
```python
from {{ pkg }} import Engine

engine = Engine.open("data.duckdb")
for row in engine.query("SELECT * FROM commits LIMIT 5"):
    print(row)
```
<!-- fl:lang ruby -->
```ruby
require "{{ pkg }}"

engine = {{ pkg }}::Engine.open("data.duckdb")
engine.query("SELECT * FROM commits LIMIT 5").each do |row|
  p row
end
```
<!-- fl:end -->

Run it with the {{ lang }} toolchain and you should see five rows.

<!-- fl:only rust -->
> Note: the Rust crate is synchronous — the query returns an iterator, no
> runtime required.
<!-- fl:end -->
<!-- fl:only node python ruby -->
> Note: in {{ lang }} the query runs off-thread, so nothing blocks the caller.
<!-- fl:end -->

## Learn more

See the full reference for the rest of the `{{ catalog.name }}` op surface.
