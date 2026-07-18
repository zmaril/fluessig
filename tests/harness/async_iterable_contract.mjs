// Hand-run harness: the async-iterable CONTRACT the Node stream codegen targets.
//
// fluessig only EMITS Rust source (napi macros); it cannot build a native addon
// here. So this script validates the observable JS contract the generated
// `#[napi(async_iterator)]` class must satisfy, using a MOCK async-iterable
// backed by a fake poll source with the same semantics as the core
// `PollStream::poll` + `close()` primitive. See README.md for how to run the
// same checks against a REAL built addon.
//
// Run:  node tests/harness/async_iterable_contract.mjs
//
// The mock mirrors the generated class:
//   - `[Symbol.asyncIterator]()` returns a fresh generator object with
//     `next()` / `return()` (matching napi 3's create_async_iterator).
//   - `next()` drives a blocking-style poll: Item -> {value, done:false},
//     Idle -> keep polling, Closed -> {value:undefined, done:true}.
//   - `return()` (triggered by `break`/early exit in for-await) calls the core
//     `close()` exactly once, idempotently.
//   - a source error rejects the awaited `next()`, so `for await` throws.

import assert from "node:assert";

// A fake core poll source with the same shape as `PollStream`:
//   poll() -> { kind: "item", value } | { kind: "idle" } | { kind: "closed" }
//                                      | { kind: "failed", error }
// plus an idempotent close() that counts its calls.
function makeSource(script) {
  let i = 0;
  let closed = 0;
  return {
    poll() {
      if (i >= script.length) return { kind: "closed" };
      return script[i++];
    },
    close() {
      closed += 1;
    },
    closeCount() {
      return closed;
    },
  };
}

// The mock async-iterable — the observable contract the generated class emits.
function makeStream(source) {
  return {
    [Symbol.asyncIterator]() {
      // A distinct generator object (napi returns a fresh one), NOT `this`.
      let done = false;
      return {
        async next() {
          // Drive the blocking poll off-thread (spawn_blocking analogue). We
          // model the poll loop: Idle -> keep going, Item -> yield, Closed/end
          // -> done, Failed -> reject.
          while (true) {
            if (done) return { value: undefined, done: true };
            const p = source.poll();
            if (p.kind === "item") return { value: p.value, done: false };
            if (p.kind === "idle") continue;
            if (p.kind === "closed") {
              done = true;
              return { value: undefined, done: true };
            }
            if (p.kind === "failed") {
              done = true;
              // Reject the awaited next(): `for await` throws. (This models a
              // poll-level failure; the gap-4 errors-as-events contract will
              // instead surface a terminal error EVENT via `item`.)
              throw p.error;
            }
          }
        },
        async return() {
          // Consumer cancelled (e.g. `break` in for-await): close the core.
          done = true;
          source.close();
          return { value: undefined, done: true };
        },
      };
    },
  };
}

// Poll-script fixture constructors — keep the object literals in one place so
// each case reads as data, not repeated boilerplate.
const item = (value) => ({ kind: "item", value });
const closed = { kind: "closed" };

// Wire a case: a fake poll source over `script`, the mock stream on top, and a
// sink array for what `for await` observes.
function wire(script) {
  const source = makeSource(script);
  return { source, stream: makeStream(source), seen: [] };
}

let failures = 0;
function pass(name) {
  console.log(`PASS: ${name}`);
}
function fail(name, err) {
  failures += 1;
  console.error(`FAIL: ${name}`);
  console.error(err && err.stack ? err.stack : err);
}

// Case (a): for-await consumes all events IN ORDER and stops at done.
async function caseOrder() {
  const name = "for-await consumes all events in order and stops at done";
  // idle polls are transparently skipped between items.
  const { source, stream, seen } = wire([item("a"), { kind: "idle" }, item("b"), item("c"), closed]);
  for await (const ev of stream) seen.push(ev);
  assert.deepStrictEqual(seen, ["a", "b", "c"], "events arrive in order");
  assert.strictEqual(source.closeCount(), 0, "natural exhaustion does not close via return()");
  pass(name);
}

// Case (b): early `break` triggers return() -> core close() exactly once.
async function caseBreak() {
  const name = "early break calls core close() exactly once and stops cleanly";
  const { source, stream, seen } = wire([item("a"), item("b"), item("c"), closed]);
  for await (const ev of stream) {
    seen.push(ev);
    if (ev === "b") break; // cancellation
  }
  assert.deepStrictEqual(seen, ["a", "b"], "stops at the break");
  assert.strictEqual(source.closeCount(), 1, "return() closed the core exactly once");
  pass(name);
}

// Case (c): a source error rejects the awaited next() so for-await throws.
async function caseError() {
  const name = "source error rejects awaited next() so for-await throws";
  const boom = new Error("poll source exploded");
  const { stream, seen } = wire([item("a"), { kind: "failed", error: boom }]);
  let threw = null;
  try {
    for await (const ev of stream) seen.push(ev);
  } catch (e) {
    threw = e;
  }
  assert.deepStrictEqual(seen, ["a"], "events before the error still arrive");
  assert.strictEqual(threw, boom, "for-await rethrows the source error");
  pass(name);
}

async function main() {
  for (const [name, fn] of [
    ["order", caseOrder],
    ["break", caseBreak],
    ["error", caseError],
  ]) {
    try {
      await fn();
    } catch (e) {
      fail(name, e);
    }
  }
  if (failures > 0) {
    console.error(`\n${failures} case(s) failed.`);
    process.exit(1);
  }
  console.log("\nAll async-iterable contract cases passed.");
}

main();
