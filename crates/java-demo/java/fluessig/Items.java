package fluessig;

import java.util.Optional;

/** Poll-based cursor over `Store.items` — call {@link #next()} until it
 * returns an empty Optional (clean close); a terminal core failure throws. */
public final class Items {
    static { System.loadLibrary("fluessig"); }

    private long cursor;

    Items(long cursor) { this.cursor = cursor; }

    private static native Object poll(long cursor);
    private static native void free(long cursor);

    /** The next item, or empty once the stream is exhausted. */
    public Optional<Item> next() {
        Object o = poll(this.cursor);
        return Optional.ofNullable((Item) o);
    }

    /** Release the cursor's core-side resources (idempotent). */
    public void close() { if (this.cursor != 0) { free(this.cursor); this.cursor = 0; } }
}
