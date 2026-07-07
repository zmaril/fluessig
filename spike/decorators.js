// No-op impls: TypeSpec records decorator applications + args on the checked
// program regardless of what the implementation does. Shared by spike/entl.tsp
// (the demo) and ../entl.tsp (the full catalog) — superset of both vocabularies.
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
