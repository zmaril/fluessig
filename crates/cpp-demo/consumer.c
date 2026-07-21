/* A real C round-trip through the generated C ABI (`cpp_demo.h`), linked against
 * the cdylib. Drives the stateful Store handle: create, put/get, list keys, hit
 * the reachable error path, bulk-remove, free — asserting every result. Then the
 * Ticker callback + subscription round-trip: a C function is registered as a
 * listener (via a fn-ptr + ctx) and fired FROM RUST across the C ABI. Prints
 * `C consumer OK` and returns 0 on success; nonzero on any mismatch. */

#include "cpp_demo.h"

#include <assert.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>

/* The listener's context: an array the C callback appends each tick value to. */
typedef struct {
    int32_t vals[16];
    size_t len;
} Seen;

/* The host callback fired from Rust: it receives its `ctx` back + the tick value
 * and records it. This proves a plain C function is invoked from the Rust core
 * through the generated fn-ptr + ctx callback ABI. */
static void on_tick_cb(void* ctx, int32_t v) {
    Seen* seen = (Seen*)ctx;
    if (seen->len < 16) {
        seen->vals[seen->len++] = v;
    }
}

int main(void) {
    char* err = NULL;

    /* ctor: an opaque handle via `Store_new` (fallible — carries err_out). */
    Store* s = NULL;
    int rc = Store_new(4, &s, &err);
    assert(rc == 0 && "Store_new should succeed for a positive capacity");
    assert(s != NULL);
    assert(err == NULL && "success leaves err_out NULL");

    /* put: string IN x2, int OUT (the new size), fallible. */
    int32_t size = -1;
    rc = Store_put(s, "alpha", "1", &size, &err);
    assert(rc == 0 && err == NULL);
    assert(size == 1);
    rc = Store_put(s, "beta", "2", &size, &err);
    assert(rc == 0 && size == 2);
    /* overwrite an existing key keeps the size. */
    rc = Store_put(s, "alpha", "11", &size, &err);
    assert(rc == 0 && size == 2);

    /* get: string -> string, fallible; the happy path. */
    char* val = NULL;
    rc = Store_get(s, "alpha", &val, &err);
    assert(rc == 0 && err == NULL);
    assert(val != NULL && strcmp(val, "11") == 0);
    fl_string_free(val);

    /* count: infallible int returned directly. */
    assert(Store_count(s) == 2);

    /* contains: infallible bool, string IN. */
    assert(Store_contains(s, "beta") == true);
    assert(Store_contains(s, "missing") == false);

    /* keys: infallible list<string> OUT via FlStringList (sorted). */
    FlStringList keys = {0};
    Store_keys(s, &keys);
    assert(keys.len == 2);
    assert(strcmp(keys.data[0], "alpha") == 0);
    assert(strcmp(keys.data[1], "beta") == 0);
    fl_string_list_free(&keys);

    /* remove_all: infallible, list<string> IN + int returned directly. Only the
     * two present keys are actually removed. */
    const char* to_remove[] = {"alpha", "gamma", "beta"};
    int32_t removed = Store_remove_all(s, to_remove, 3);
    assert(removed == 2);
    assert(Store_count(s) == 0);

    /* the ERROR path: a missing key gives nonzero status + an owned err string,
     * freed with fl_error_free. */
    val = NULL;
    err = NULL;
    rc = Store_get(s, "nope", &val, &err);
    assert(rc != 0 && "a missing key is an error");
    assert(err != NULL && "the error message is handed back through err_out");
    assert(strstr(err, "nope") != NULL && "the message names the missing key");
    fl_error_free(err);

    /* lifecycle: release the handle. */
    Store_free(s);

    /* ── Ticker: the callback + subscription round-trip ── */
    Ticker* t = NULL;
    err = NULL;
    rc = Ticker_new(&t, &err);
    assert(rc == 0 && t != NULL && err == NULL);

    /* subscribe a C function (fn-ptr + ctx); on_tick is fallible → out + err_out. */
    Seen seen = {0};
    Subscription* sub = NULL;
    err = NULL;
    rc = Ticker_on_tick(t, on_tick_cb, &seen, &sub, &err);
    assert(rc == 0 && sub != NULL && err == NULL);

    /* tick twice: the host callback fires from Rust with the incrementing counter. */
    Ticker_tick(t);
    Ticker_tick(t);
    assert(seen.len == 2 && seen.vals[0] == 0 && seen.vals[1] == 1);

    /* unsubscribe, then tick again: the listener is gone, so nothing is recorded. */
    Subscription_unsubscribe(sub);
    Ticker_tick(t);
    assert(seen.len == 2 && "unsubscribe removes the listener");

    /* lifecycle: free the subscription (idempotent unsubscribe) + the ticker. */
    Subscription_free(sub);
    Ticker_free(t);
    printf("C callback fired with [%d, %d] then stayed silent after unsubscribe\n",
           seen.vals[0], seen.vals[1]);

    printf("C consumer OK\n");
    return 0;
}
