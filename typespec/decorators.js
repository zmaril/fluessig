// No-op impls: TypeSpec records decorator applications + args on the checked
// program regardless of what the implementation does. Validation lives in the
// Rust catalog loader (DESIGN §4 — the emitter/library side stays dumb).
function noop() {}

export const $decorators = {
  Fluessig: {
    entity: noop,
    abstract: noop,
    edge: noop,
    compose: noop,
    name: noop,
    fk: noop,
    fkSource: noop,
    defaultValue: noop,
    derived: noop,
    ctor: noop,
    stream: noop,
    manual: noop,
    readonly: noop,
    destructive: noop,
  },
};
