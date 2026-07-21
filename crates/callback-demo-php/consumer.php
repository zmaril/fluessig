<?php
// The PHP host consumer for the callback + subscription demo. Loads the built
// ext-php-rs extension, subscribes a PHP Closure via `onTick`, and asserts the
// Closure is invoked from the Rust core with the incrementing counter — then goes
// silent once the Subscription is unsubscribed.
//
// `onTick($listener)` lowers `$listener` to the uniform core `Box<dyn Fn(i32)>`
// (via the sync-only `PhpCb` newtype); `tick` fires every live listener
// SYNCHRONOUSLY on this (the PHP request) thread, so no event-loop drain is needed
// — the values have landed by the time `tick` returns. PHP callbacks are
// documented SYNC-ONLY: the closure is only ever invoked here, on the request
// thread that supplied it.

declare(strict_types=1);

$ticker = new Ticker();
$seen = [];
$sub = $ticker->onTick(function (int $v) use (&$seen) { $seen[] = $v; });

$ticker->tick(); // fires 0
$ticker->tick(); // fires 1
if ($seen !== [0, 1]) {
    fwrite(STDERR, "expected [0, 1] before unsubscribe, saw " . json_encode($seen) . "\n");
    exit(1);
}
echo "ticks-before-unsub=" . json_encode($seen) . "\n";

$sub->unsubscribe();
$ticker->tick(); // fires 2 to nobody — the listener is gone
if ($seen !== [0, 1]) {
    fwrite(STDERR, "expected [0, 1] after unsubscribe, saw " . json_encode($seen) . "\n");
    exit(1);
}
echo "ticks-after-unsub=" . json_encode($seen) . "\n";

echo "callback-ok: PHP Closure fired [0, 1] from Rust (sync on the request thread), silent after unsubscribe\n";
