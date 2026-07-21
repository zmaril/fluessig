# frozen_string_literal: true

# The Ruby host consumer for the callback + subscription demo. Loads the built
# magnus extension, subscribes a Ruby Proc via `on_tick`, and asserts the Proc is
# invoked from the Rust core with the incrementing counter — then goes silent once
# the Subscription is unsubscribed.
#
# `on_tick(listener)` lowers `listener` to the uniform core `Box<dyn Fn(i32)>`;
# `tick` fires every live listener SYNCHRONOUSLY on this (the Ruby) thread under
# the GVL, so no event-loop drain is needed — the values have landed by the time
# `tick` returns.

require "callback_demo_ruby"

ticker = Ticker.new
seen = []
sub = ticker.on_tick(->(v) { seen << v })

ticker.tick # fires 0
ticker.tick # fires 1
raise "expected [0, 1] before unsubscribe, saw #{seen.inspect}" unless seen == [0, 1]
puts "ticks-before-unsub=#{seen.inspect}"

sub.unsubscribe
ticker.tick # fires 2 to nobody — the listener is gone
raise "expected [0, 1] after unsubscribe, saw #{seen.inspect}" unless seen == [0, 1]
puts "ticks-after-unsub=#{seen.inspect}"

puts "callback-ok: Ruby Proc fired [0, 1] from Rust, silent after unsubscribe"
