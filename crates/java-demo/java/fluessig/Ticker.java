package fluessig;

/** A stateful ticker that fires registered listeners with an incrementing counter. `on_tick` is a `Shape::Subscription` op — it REGISTERS a host callback (a Java `Consumer<Integer>`) and hands back an owning `Subscription` handle whose unsubscribe()/close() removes the listener; `tick` fires every live listener. This is the callback + subscription slice's Java round-trip proof. */
public final class Ticker {
    static { System.loadLibrary("fluessig"); }

    private static native long init();
    private static native void free(long handle);
    private static native long nativeOnTick(long handle, java.util.function.Consumer<Integer> listener);
    private static native void tick(long handle);

    private long handle;

    /** Construct the `Ticker` handle. */
    public Ticker() { this.handle = init(); }

    /** Release the handle (idempotent). */
    public void close() { if (this.handle != 0) { free(this.handle); this.handle = 0; } }

    /** Register a listener on `Ticker.on_tick`; returns an owning Subscription. */
    public Subscription onTick(java.util.function.Consumer<Integer> listener) { return new Subscription(nativeOnTick(this.handle, listener)); }

    /** `Ticker.tick`. */
    public void tick() { tick(this.handle); }

}
