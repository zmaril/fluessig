// straitjacket-allow-file:duplication — the generated per-interface JNI classes repeat a fixed handle/ctor/close + native-decl template by design (the language × shape grid).
package fluessig;

import java.util.concurrent.CompletableFuture;

/** An open demo store. Exercises one op of every shape the Java backend projects. */
public final class Store {
    static { System.loadLibrary("fluessig"); }

    private static native long init(long seed);
    private static native void free(long handle);
    private static native String version(long handle);
    private static native long checked(long handle, String key);
    private static native long nativeCount(long handle, String prefix);
    private static native long nativeItems(long handle);

    private long handle;

    /** Construct the `Store` handle. */
    public Store(long seed) { this.handle = init(seed); }

    /** Release the handle (idempotent). */
    public void close() { if (this.handle != 0) { free(this.handle); this.handle = 0; } }

    /** `Store.version`. */
    public String version() { return version(this.handle); }

    /** `Store.checked`. */
    public long checked(String key) { return checked(this.handle, key); }

    /** Async `Store.count` — the blocking native call wrapped in a future. */
    public CompletableFuture<Long> count(String prefix) {
        return CompletableFuture.supplyAsync(() -> nativeCount(this.handle, prefix));
    }

    /** Open the `Store.items` stream as a poll cursor. */
    public Items items() { return new Items(nativeItems(this.handle)); }

}
