# Consumer Fiber control is a FiberHandle, not a Fork

Status: accepted

Cordis names the cloneable public control capability for one non-root Fiber
`FiberHandle`. The previous `Fork` name suggested branching, parenthood, or a
lifetime relationship that the v3 model explicitly rejects: spawn origin is
provenance only, Runtime/Registry residency keeps the Fiber alive, and dropping
all consumer handles is inert. `FiberId` remains correlation-only identity,
while `FiberHandle` is the lifecycle-control surface. The old `Fork` spelling is
retired rather than retained as a compatibility alias so one canonical domain
term appears throughout the public API, Loader outcomes, examples, and docs.

## Considered options

- `Fork`: rejected because it implies a branch/parent relation that does not
  exist in the runtime model.
- `FiberHandler`: rejected because the value does not handle Fiber events or
  requests; it is a handle *to* a Fiber.
- `FiberRef`: rejected because the value carries lifecycle control authority,
  not merely read/reference semantics.
