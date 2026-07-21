//! The hand-written engine behind the `Ticker` op surface — the ONE module a
//! consumer writes by hand (the house-style "generated surface + hand-written
//! `core_impl`" split). Everything else in the crate is generated.
//!
//! `each_tick` is the whole point of the callback slice: the core invokes the
//! host-supplied closure `count` times, straight from Rust, with an incrementing
//! counter. It never learns whether that closure came from JS, Python, or Rust —
//! it just sees the ONE uniform `Box<dyn Fn(i32) + Send + Sync>` shape.
//!
//! straitjacket-allow-file:duplication — deliberately parallel to the sibling
//! `crates/callback-demo-py/src/core_impl.rs`; each demo crate carries its own
//! `TickerImpl` so the node and python round-trips are independently buildable.

use crate::generated::TickerCore;

/// A stateless ticker. `each_tick` is a free-function-style op (no ctor), so the
/// impl carries no state.
pub struct TickerImpl;

impl TickerCore for TickerImpl {
    fn each_tick(count: i32, listener: Box<dyn Fn(i32) + Send + Sync>) -> anyhow::Result<()> {
        for i in 0..count {
            listener(i);
        }
        Ok(())
    }
}
