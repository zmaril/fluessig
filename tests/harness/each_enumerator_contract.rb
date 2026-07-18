#!/usr/bin/env ruby
# frozen_string_literal: true

# Stream each/Enumerator — hand-run harness (Ruby analogue of the .mjs / .py ones).
#
# Validates the OBSERVABLE Ruby contract the Magnus stream codegen targets: the
# `#[magnus::wrap]` class fluessig emits for every `stream` op must behave, from
# Ruby, like a well-formed `each`-able — `stream.each { |ev| ... }` yields each
# event to the block, and `stream.each` with NO block returns an `Enumerator`
# (so `.lazy` / `.map` / `.next` compose). It is backed by a fake poll source
# with the same semantics as the core `PollStream::poll` / `close()` primitive.
#
# fluessig is a codegen tool: `ruby_binding()` emits Rust source (with Magnus
# macros) as a String; it never compiles that source and cannot build a native
# Ruby extension in its own CI. So this harness mocks the exact observable
# contract in pure Ruby rather than importing a real generated class.
#
# Run: `ruby tests/harness/each_enumerator_contract.rb`
# Plain `ruby`, no gems, no build step. Prints `PASS:` per case, exits 1 on fail.

# ── Poll kinds — the four arms of the core `Poll<T>` enum ──────────────────────
ITEM = :item
IDLE = :idle
CLOSED = :closed
FAILED = :failed

# A fake poll source: replays a scripted sequence of poll results, and counts
# `close` calls (idempotent — a second close is a no-op, mirroring the core
# `PollStream::close` default).
class FakePollSource
  attr_reader :close_count

  def initialize(script)
    @script = script.dup
    @closed = false
    @close_count = 0
  end

  def poll
    return [CLOSED, nil] if @closed
    @script.empty? ? [CLOSED, nil] : @script.shift
  end

  def close
    return if @closed
    @closed = true
    @close_count += 1
  end
end

# The mock of the generated wrap class. `each` mirrors the emitted Rust `each`:
# no block => an Enumerator over `each`; with a block => yield each Poll::Item,
# skip Poll::Idle, end on Poll::Closed, and on Poll::Failed either RAISE
# (throw-mode) or YIELD a terminal error event then END (event-mode).
#
# The generated Rust closes the core in `impl Drop`; Ruby has no deterministic
# destructor, so this harness models that backstop with an `ensure` — the same
# guarantee (the core is closed exactly once when iteration is abandoned or
# finished), which is what the `break` case asserts.
class Stream
  def initialize(source, mode: :throw, tag_value: "error")
    @source = source
    @mode = mode
    @tag_value = tag_value
  end

  def close
    @source.close
  end

  def each
    return enum_for(:each) unless block_given?

    begin
      loop do
        kind, value = @source.poll
        case kind
        when ITEM then yield value
        when IDLE then next
        when CLOSED then break
        when FAILED
          if @mode == :throw
            raise RuntimeError, value # throw-mode: raises out of `each`
          else
            # event-mode: hand the failure out AS the terminal event, then END.
            yield({ "type" => @tag_value, "reason" => "error", "error" => value })
            break
          end
        end
      end
      self # `each` returns the receiver, like Array#each
    ensure
      close # Drop-backstop analogue (see class comment)
    end
  end
end

# ── the harness ───────────────────────────────────────────────────────────────
failures = 0

def check(name, cond)
  if cond
    puts "PASS: #{name}"
  else
    puts "FAIL: #{name}"
    return 1
  end
  0
end

# Case 1 — order: block form consumes all events in order, skips idle polls,
# stops at Closed.
begin
  src = FakePollSource.new([
                             [ITEM, "a"], [IDLE, nil], [ITEM, "b"], [ITEM, "c"], [CLOSED, nil]
                           ])
  got = []
  ret = Stream.new(src).each { |ev| got << ev }
  failures += check("order: consumes all events in order, idle skipped", got == %w[a b c])
  failures += check("order: each returns the receiver (a Stream)", ret.is_a?(Stream))
  failures += check("order: core closed once at end", src.close_count == 1)
rescue StandardError => e
  failures += check("order: no exception (#{e.class}: #{e.message})", false)
end

# Case 2 — no-block form returns an Enumerator (so .lazy/.map/.next compose).
begin
  src = FakePollSource.new([[ITEM, 1], [ITEM, 2], [ITEM, 3], [CLOSED, nil]])
  e = Stream.new(src).each
  failures += check("enumerator: no-block each returns an Enumerator", e.is_a?(Enumerator))
  failures += check("enumerator: .next pulls the first event", e.next == 1)
  failures += check("enumerator: .lazy composes off the Enumerator",
                    Stream.new(FakePollSource.new([[ITEM, 1], [ITEM, 2], [ITEM, 3],
                                                   [CLOSED, nil]])).each.lazy.map { |x| x * 10 }.first(2) == [10, 20])
rescue StandardError => e
  failures += check("enumerator: no exception (#{e.class}: #{e.message})", false)
end

# Case 3 — early break triggers the core close exactly once.
begin
  src = FakePollSource.new([[ITEM, "a"], [ITEM, "b"], [ITEM, "c"], [CLOSED, nil]])
  seen = []
  Stream.new(src).each do |ev|
    seen << ev
    break if ev == "a"
  end
  failures += check("break: stops early (only the first event consumed)", seen == %w[a])
  failures += check("break: core closed exactly once", src.close_count == 1)
rescue StandardError => e
  failures += check("break: no exception (#{e.class}: #{e.message})", false)
end

# Case 4 — throw-mode: a mid-stream failure raises out of `each`.
begin
  src = FakePollSource.new([[ITEM, "a"], [FAILED, "boom"]])
  raised = false
  begin
    Stream.new(src, mode: :throw).each { |_ev| } # rubocop:disable Lint/EmptyBlock
  rescue RuntimeError => e
    raised = e.message == "boom"
  end
  failures += check("throw: mid-stream failure raises out of each", raised)
  failures += check("throw: core still closed once (ensure backstop)", src.close_count == 1)
rescue StandardError => e
  failures += check("throw: unexpected exception (#{e.class}: #{e.message})", false)
end

# Case 5 — event-mode: a mid-stream failure is yielded as a terminal error event
# then the block ends; it NEVER raises (the @streamError dual-error split).
begin
  src = FakePollSource.new([[ITEM, "a"], [FAILED, "boom"], [ITEM, "unreached"]])
  got = []
  Stream.new(src, mode: :event).each { |ev| got << ev }
  terminal = got.last
  failures += check("event: item then a terminal error event, no raise",
                    got.length == 2 && got.first == "a" && terminal.is_a?(Hash))
  failures += check("event: terminal event is { type:, reason:, error: }",
                    terminal == { "type" => "error", "reason" => "error", "error" => "boom" })
  failures += check("event: stream ended after the terminal event (unreached not seen)",
                    got.none? { |ev| ev == "unreached" })
rescue StandardError => e
  failures += check("event: must not raise (#{e.class}: #{e.message})", false)
end

puts(failures.zero? ? "\nALL PASS" : "\n#{failures} FAILURE(S)")
exit(failures.zero? ? 0 : 1)
