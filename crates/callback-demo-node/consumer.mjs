// The node host consumer for the callback demo. Loads the built .node addon,
// passes a JS closure into the generated `eachTick`, and asserts the closure was
// invoked from Rust with [0, 1, 2].
//
// `eachTick(count, listener)` lowers `listener` to a napi `ThreadsafeFunction`
// called `NonBlocking`, so each invocation is QUEUED onto the JS event loop
// rather than run inline. We therefore drain the loop (`await setImmediate`)
// before asserting — by then every queued call has landed.

import { createRequire } from "node:module";
import assert from "node:assert";
import { fileURLToPath } from "node:url";
import path from "node:path";

const require = createRequire(import.meta.url);
const here = path.dirname(fileURLToPath(import.meta.url));
const addonPath = process.env.CALLBACK_ADDON ?? path.join(here, "callback_demo_node.node");

const { eachTick } = require(addonPath);

const seen = [];
eachTick(3, (v) => seen.push(v));

// TSFN NonBlocking delivers on the next event-loop turn(s); drain before asserting.
await new Promise((resolve) => setImmediate(resolve));

assert.deepStrictEqual(seen, [0, 1, 2], `expected [0,1,2], got ${JSON.stringify(seen)}`);
console.log("node callback fired:", seen);
