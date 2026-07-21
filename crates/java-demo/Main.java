// The round-trip driver: load the fluessig cdylib and call one op of every shape
// the Java (JNI) backend projects, printing exact, order-sensitive lines the
// runner asserts against. Nothing here is generated — it is the "consumer" a
// real caller would write against the generated `fluessig.*` classes.

import fluessig.Store;
import fluessig.Item;
import fluessig.Items;
import fluessig.Ticker;
import fluessig.Subscription;

import java.util.ArrayList;
import java.util.List;
import java.util.Optional;

public class Main {
    public static void main(String[] args) throws Exception {
        // ctor → the stateful native handle (init/free).
        Store store = new Store(100);

        // sync + infallible: a direct blocking native returning a bare String.
        System.out.println("version=" + store.version());

        // sync + fallible (Ok path): a blocking native, no exception thrown.
        System.out.println("checked(abc)=" + store.checked("abc"));

        // async: the blocking native wrapped in a CompletableFuture.
        System.out.println("count(stream)=" + store.count("stream").get());

        // stream: drain the poll cursor to its clean close (empty Optional).
        Items items = store.items();
        Optional<Item> it;
        while ((it = items.next()).isPresent()) {
            Item item = it.get();
            System.out.println("item " + item.getId() + " " + item.getLabel());
        }
        items.close();
        System.out.println("stream-closed");

        // sync + fallible (Err path): the core's Err becomes a thrown
        // RuntimeException across the JNI seam.
        try {
            store.checked("boom");
            System.out.println("throw-FAILED: no exception thrown");
        } catch (RuntimeException e) {
            System.out.println("throw-ok: " + e.getMessage());
        }

        store.close();

        // ── callback + subscription: a real Java Consumer fired from Rust ──
        // Register a Consumer<Integer> that records every value it sees. `tick()`
        // fires the listener from the Rust core with an incrementing counter;
        // unsubscribe() removes it, so later ticks are silent.
        Ticker ticker = new Ticker();
        List<Integer> seen = new ArrayList<>();
        Subscription sub = ticker.onTick(seen::add);

        ticker.tick(); // fires 0
        ticker.tick(); // fires 1
        if (!seen.equals(List.of(0, 1))) {
            throw new AssertionError("expected [0, 1] before unsubscribe, saw " + seen);
        }
        System.out.println("ticks-before-unsub=" + seen);

        sub.unsubscribe();
        ticker.tick(); // fires 2 to nobody — the listener is gone
        if (!seen.equals(List.of(0, 1))) {
            throw new AssertionError("expected [0, 1] after unsubscribe, saw " + seen);
        }
        System.out.println("ticks-after-unsub=" + seen);

        sub.close();
        ticker.close();
        System.out.println("callback-ok: Java Consumer fired [0, 1] from Rust, silent after unsubscribe");
    }
}
