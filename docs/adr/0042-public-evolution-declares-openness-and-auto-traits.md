# Public evolution declares openness and auto traits

Status: accepted

The compatibility policy listed what breaks the supported surface but not what
may be added compatibly, and the normative public interface did not say which
vocabularies may grow or which auto traits consumers may rely on. Before the
API freeze, the supported surface declares four things explicitly. The rules
are stated in the
[compatibility policy](../compatibility-policy.md#compatible-evolution) and the
[public interface](../v3-public-interface.md#openness-and-auto-traits); this
decision records why.

1. **Every public enum declares its openness.** An Open enum is
   `#[non_exhaustive]` and may gain variants in a minor release. A Closed enum
   gains a variant only in a semver-incompatible release. Thirteen
   vocabularies that the authority expects to grow were opened for this
   (`LifecycleOperation`, `PluginFailureKind`, `EffectFailureKind`,
   `InvocationFailureKind`, `ResolverFailureKind`, `DispatchOutcomeKind`,
   `EventOperation`, `ListenerRole`, `Routing`, `RuntimeObservation`,
   `ObservationRouting`, `EntryOutcome`, and `RealmPolicy`). With the operation
   error enums, which were already `#[non_exhaustive]`, 32 of the 40 public
   enums are Open. The ten output-only record variants of `RuntimeObservation`
   and `EntryOutcome` are also `#[non_exhaustive]`, so they may gain fields.
   Eight enums are Closed on purpose:

   | Closed enum | Why it is closed |
   | --- | --- |
   | `FiberState` | The six states define the generation gate, `ready()`'s answer set, and `wait_state` targets. A seventh state is an architecture change that breaks every consumer whatever the attribute, so exhaustive matching is the useful signal. |
   | `FiberRole` | Root and ordinary partition every Fiber by definition, with one permanent root. |
   | `UpdateOutcome` | A commit / no-commit partition; every other ending is the Open `UpdateError`. |
   | `QueryOutcome<T>` | Presence is binary (`Miss` or `Answer`) and failure is `DispatchError`. Another result shape would be another operation. |
   | `ResidencyChange` | A binary membership transition (admitted or removed). |
   | `ListenerChange` | A binary membership transition (registered or unregistered). |
   | `InjectEntry` | Every resolver must handle every inject kind to complete the overlay, so a new kind breaks the resolver contract whatever the attribute. Exhaustive matching is the resolver author's signal. |
   | `TimeoutOutcome<T>` | A binary race outcome; cancellation is the separate `TimerCancelled`. |

2. **The fields of error variants are frozen.** The 17 struct-like variants of
   the Open error enums keep exhaustive fields, and so do the constructible
   source variants `InjectEntry::Configured` and `RealmPolicy::Shared`. New
   failure detail arrives as a new variant of the Open enum, never as a new
   field.

3. **Auto traits are promised by list.** `Context`, `FiberHandle`, every public
   error and failure type, the correlation identities, the snapshots, the
   observation records, and the outcomes are `Send + Sync`. Generic ones are
   `Send + Sync` when their parameter is. The Timer operation futures are
   `Send` when their inner future is `Send`. `Unpin` is never promised, and no
   auto trait outside the list is promised, even where rustdoc shows it. UI pass
   fixtures in `cordis-core`, `cordis-loader`, and `cordis-timer`
   (`tests/ui-auto-traits/pass/`) assert every promised auto trait.

4. **Listener-adapter bounds are governed like signatures.** The bounds under
   which `observer`, `observer_sync`, `responder`, `responder_sync`, `mapper`,
   `mapper_sync`, `around`, and `with_state` accept a callback live on blanket
   implementations of traits that downstream code cannot name. The set of
   callbacks each adapter accepts is nevertheless a supported signature:
   tightening those bounds is breaking, and loosening them is compatible when
   existing callers' inference is unaffected.

The trait-evolution rules in the policy follow from the same review. No open
trait can gain an associated type, because associated type defaults are
unstable. `Exporter` and `PluginResolver` are used as trait objects by
supported signatures, so they can gain only dyn-compatible provided methods
and no associated constant.

## Considered options

- **Leave every enum exhaustive and declare them all closed**: rejected. The
  architecture names reopening conditions (Runtime-wide shutdown, a structured
  Fiber owner, a non-tree Event audience, a named-realm rendezvous) and ADR 0041
  expects stronger destructor promises later. Each of those needs a new
  variant, so each would force a new major line.
- **Open every enum**: rejected for the eight rows above. For them a new
  variant changes the meaning of every existing match, so `#[non_exhaustive]`
  would hide a semantic break behind a wildcard arm.
- **Open the fields of error variants too**: rejected. It would break 38
  in-repository patterns and constructions for diagnostic growth that a new
  variant expresses equally well.
- **Promise every auto trait rustdoc shows**: rejected. `Sleep`, `Interval`, and
  `Timeout<F>` are `Unpin` today only because of their current representation.
  Promising it would freeze that representation.
- **Promise no auto trait**: rejected. `Context::run` and
  `Context::spawn_attributed` accept only `Send` futures, so a task that
  captures a `Context` or `FiberHandle`, or holds an `Interval` across an
  await, relies on those auto traits. The `worker_daemon` example does exactly
  this. Leaving them unstated would leave a relied-upon property unguarded.

## Consequences

- The interface carries one openness table covering every public enum, and one
  auto-trait list. Adding a public enum or a promised type updates them in the
  same change.
- Removing a promised auto trait is a breaking change and fails a UI pass
  fixture. Adding a promise for a type that already has the trait is additive.
- Consumers match Open enums with a wildcard arm and Open variants with `..`.
  They may match Closed enums exhaustively and treat a new Closed variant as a
  major-version signal.
- Richer failure diagnostics arrive as new error variants.
