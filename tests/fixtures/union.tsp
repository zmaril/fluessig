// The tagged-union exercise catalog (format 1): a disponent-shaped event
// stream — one union, stored on an entity (twin columns), crossing the op
// surface (envelope carrier), with variants pulled into the api closure.
import "../../typespec/lib.tsp";
using Fluessig;

/** One observation's body — the union's variant tag is the wire discriminator. */
union EventPayload {
  message: AgentMessage,
  log: LogLine,
  exit: ExitInfo,
}

/** Something an agent said. */
model AgentMessage {
  role: string;
  text: string;
}

model LogLine {
  line: string;
}

model ExitInfo {
  code: int32;
  reason?: string;
}

@entity
@name("events")
model Event {
  @key sessionUid: string;
  @key idx: int64;
  ts: utcDateTime;
  /** The observation body (twin columns: payload_kind + payload). */
  payload: EventPayload;
  /** A nullable union — both twins nullable. */
  note?: EventPayload;
}

interface Watch {
  @ctor open(path: string): void;
  @stream events(afterIdx?: int64): Event;
  emit(payload: EventPayload): void;
}
