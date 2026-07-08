# entl — Python quickstart

`entl` describes a typed entity graph once and projects it
everywhere. This guide shows the Python surface, generated from one template.

## Install

```sh
uv pip install entl
```

## Quickstart

Open the engine, run one query, and print the rows. Save this as
`quickstart.py`.

```python
from entl import Engine

engine = Engine.open("data.duckdb")
for row in engine.query("SELECT * FROM commits LIMIT 5"):
    print(row)
```

Run it with the Python toolchain and you should see five rows.

> Note: in Python the query runs off-thread, so nothing blocks the caller.

## Learn more

See the full reference for the rest of the `entl` op surface.
