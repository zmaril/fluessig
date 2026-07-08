# entl — Ruby quickstart

`entl` describes a typed entity graph once and projects it
everywhere. This guide shows the Ruby surface, generated from one template.

## Install

```sh
bundle add entl
```

## Quickstart

Open the engine, run one query, and print the rows. Save this as
`quickstart.rb`.

```ruby
require "entl"

engine = entl::Engine.open("data.duckdb")
engine.query("SELECT * FROM commits LIMIT 5").each do |row|
  p row
end
```

Run it with the Ruby toolchain and you should see five rows.

> Note: in Ruby the query runs off-thread, so nothing blocks the caller.

## Learn more

See the full reference for the rest of the `entl` op surface.

<!-- straitjacket-allow-file:duplication — parallel per-language renders of one template -->
