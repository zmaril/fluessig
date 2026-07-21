package fluessig;


/** One streamed record — a flat scalar DTO the `items` stream yields. */
public final class Item {
    private final long id;
    private final String label;

    public Item(long id, String label) {
        this.id = id;
        this.label = label;
    }

    public long getId() { return this.id; }
    public String getLabel() { return this.label; }
}
