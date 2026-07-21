// A real C++ round-trip through the generated RAII wrapper (`cpp_demo.hpp`),
// linked against the same cdylib. Drives the `fluessig::Store` class with
// std::string / std::vector, catches `fluessig::Error` on the reachable error
// path, and asserts every result. Then the Ticker callback + subscription
// round-trip: a C++ lambda is registered via the RAII `Subscription` wrapper and
// fired FROM RUST. Prints `C++ consumer OK`, returns 0 on success (nonzero on any
// mismatch).

#include "cpp_demo.hpp"

#include <cassert>
#include <cstdint>
#include <cstdio>
#include <string>
#include <vector>

int main() {
    // ctor: the RAII handle (throws fluessig::Error on failure).
    fluessig::Store store(4);

    // put: std::string IN, int OUT (the new size), throws on failure.
    assert(store.put("alpha", "1") == 1);
    assert(store.put("beta", "2") == 2);
    assert(store.put("alpha", "11") == 2); // overwrite keeps the size

    // get: std::string -> std::string, happy path.
    assert(store.get("alpha") == "11");

    // count: infallible int.
    assert(store.count() == 2);

    // contains: infallible bool.
    assert(store.contains("beta"));
    assert(!store.contains("missing"));

    // keys: infallible list<string> -> std::vector<std::string> (sorted; the C
    // buffer is freed inside the wrapper).
    std::vector<std::string> keys = store.keys();
    assert(keys.size() == 2);
    assert(keys[0] == "alpha");
    assert(keys[1] == "beta");

    // remove_all: std::vector<std::string> IN + int out; only present keys count.
    std::vector<std::string> to_remove{"alpha", "gamma", "beta"};
    assert(store.remove_all(to_remove) == 2);
    assert(store.count() == 0);

    // the ERROR path: a missing key throws fluessig::Error carrying the message.
    bool threw = false;
    try {
        store.get("nope");
    } catch (const fluessig::Error& e) {
        threw = true;
        assert(std::string(e.what()).find("nope") != std::string::npos);
    }
    assert(threw && "a missing key must throw fluessig::Error");

    // ── Ticker: the callback + subscription round-trip via the RAII wrapper ──
    fluessig::Ticker ticker;
    std::vector<int32_t> seen;
    {
        // Register a C++ lambda (captured into std::function); on_tick returns an
        // RAII Subscription whose destructor removes the listener.
        fluessig::Subscription sub =
            ticker.on_tick([&](int32_t v) { seen.push_back(v); });

        // tick twice: the lambda fires from Rust with the incrementing counter.
        ticker.tick();
        ticker.tick();
        assert((seen == std::vector<int32_t>{0, 1}));

        // unsubscribe, then tick again: the listener is gone.
        sub.unsubscribe();
        ticker.tick();
        assert((seen == std::vector<int32_t>{0, 1}));
    } // sub destructor frees the subscription (idempotent)

    std::printf("C++ callback fired with [%d, %d] then stayed silent after unsubscribe\n",
                seen[0], seen[1]);

    // the destructors call Store_free / Ticker_free — nothing to do here.
    std::printf("C++ consumer OK\n");
    return 0;
}
