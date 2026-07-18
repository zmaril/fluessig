// No-op impls: TypeSpec records decorator applications + args on the checked
// program regardless of what the implementation does. Backs the standalone
// demo catalog (entl.tsp, this dir) — a superset of the decorator vocabulary.
function noop() {}

export const $decorators = {
  Fluessig: {
    entity: noop,
    abstract: noop,
    key: noop,
    edge: noop,
    compose: noop,
    name: noop,
    fk: noop,
    fkSource: noop,
    defaultValue: noop,
    ctor: noop,
    stream: noop,
    manual: noop,
  },
};
