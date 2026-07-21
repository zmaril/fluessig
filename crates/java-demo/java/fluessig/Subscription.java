// straitjacket-allow-file:duplication — the generated per-interface JNI classes repeat a fixed handle/ctor/close + native-decl template by design (the language × shape grid).
package fluessig;

/** An opaque subscription handle. `unsubscribe()` removes the listener early
 * (idempotent); `close()` frees the native handle (also unsubscribing if still
 * live). Returned by a `Shape::Subscription` op such as `Ticker.onTick`. */
public final class Subscription {
    static { System.loadLibrary("fluessig"); }

    private long handle;

    Subscription(long handle) { this.handle = handle; }

    private static native void nativeUnsubscribe(long handle);
    private static native void nativeFree(long handle);

    /** Remove the listener early (idempotent); the handle is still freed by close(). */
    public void unsubscribe() { if (this.handle != 0) { nativeUnsubscribe(this.handle); } }

    /** Free the native handle (also unsubscribing if still live); idempotent. */
    public void close() { if (this.handle != 0) { nativeFree(this.handle); this.handle = 0; } }
}
