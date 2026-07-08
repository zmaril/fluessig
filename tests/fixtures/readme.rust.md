# entl — Rust quickstart

`entl` describes a typed entity graph once and projects it
everywhere. This guide shows the Rust surface, generated from one template.

## Install

```sh
cargo add entl
```

## Quickstart

Open the engine, run one query, and print the rows. Save this as
`quickstart.rs`.

```rust
use entl::Engine;

fn main() -> anyhow::Result<()> {
    let engine = Engine::open("data.duckdb")?;
    for row in engine.query("SELECT * FROM commits LIMIT 5")? {
        println!("{row:?}");
    }
    Ok(())
}
```

Run it with the Rust toolchain and you should see five rows.

> Note: the Rust crate is synchronous — the query returns an iterator, no
> runtime required.

## Learn more

See the full reference for the rest of the `entl` op surface.
