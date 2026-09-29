# User destructor panics are best-effort

Status: accepted

Values supplied to Cordis must not panic when dropped. This covers Plugin
values and inputs, Service values, listener, cleanup and task closures and their
captures, Event arguments and answers, exporters, observers, errors and panic
payloads. When such a destructor panics while Cordis drops the value, Cordis
makes a best-effort attempt to contain and report the panic and to continue the
surrounding framework work. It promises no specific outcome. The following are
unspecified: which operation observes or reports the diagnostic, whether an
associated answer, value or result is discarded, whether a pending operation
still reports success, and whether the panic resumes in an awaiting caller. A
destructor panic during another unwind, or in a build using `panic = "abort"`,
can abort the process.

This rule is the complete destructor-panic contract of `cordis-core` and of the
`cordis-loader` and `cordis-timer` leaves. Stronger guarantees may be added later
as compatible, additive promises.

This decision deliberately amends
[ADR 0033](0033-events-are-typed-completion-aware-and-occurrence-claimed.md). It
supersedes that ADR's rules for the destruction of an unused waterfall tail or an
uncalled continuation, and its rules for destruction of a completed invocation's
final callback reference. It also supersedes the public-interface rule that an
old `Plugin::Input` destructor panic during era swap completes final dependent
convergence before resuming in the awaiting caller. The containment code and the
regression tests those rules produced remain in place. They are now evidence of
best-effort robustness, not discriminators of a contract.

Two neighboring rules are unchanged:

- [ADR 0029](0029-lifecycle-commits-complete-and-critical-sections-are-closed.md)
  still requires that user-controlled destruction happen outside framework
  synchronization. That rule is about re-entrancy safety, and it holds whether
  or not a destructor panics.
- Every handle and registration whose Drop is documented as inert remains inert,
  including `FiberHandle`, `ListenerRegistration`, `EffectRegistration` and
  `ExporterRegistration`. Inert Drop describes what dropping a Cordis-owned
  capability does. It makes no promise about user destructors.

## Considered options

- **Point promises for each destruction site**: rejected. Before this decision,
  each containment fix added its own normative clause. The result was uneven:
  Event destruction was specified in detail, while equivalent lifecycle,
  Registry and Loader fixes had no contract text. Freezing that set would commit
  to an accidental subset. Destructor defects were still being found at a steady
  rate, so each new site would reopen the API freeze criteria.
- **Structural wrapping at every entry point**: rejected. Containing every user
  value at the point Cordis takes ownership would change core ownership paths,
  including the last `Arc<Fiber>` reference, the Input held in spawn state and
  callback snapshots. The Loom models and the ADR 0029 evidence for those paths
  would have to be re-established. It still could not cover values returned to
  the caller, a second panic during an unwind, or `panic = "abort"` builds.
- **One general best-effort rule**: chosen. It states what Cordis can promise
  honestly today, keeps every existing containment path, and leaves room to
  strengthen specific paths later.

## Rationale

Reversibility is asymmetric. Narrowing now and strengthening later is additive,
while freezing specific promises and then narrowing them would be a break. The
rule also matches Rust convention, where a panicking `Drop` is a bug in the type
that defines it. Isolating untrusted plugin code is a job for a sandbox, as
explored in the draft Wasm experiment
([#145](https://github.com/dshbox/cordis-rs/pull/145)), not for destructor
containment.

## Consequences

- The public interface states one general destructor rule. Event and era-swap
  sections refer to it instead of carrying their own destructor promises.
- `InvocationFailureKind::Panic` may report a destructor panic, but consumers
  cannot rely on one being reported there.
- Existing destructor containment code and tests are kept as best-effort
  robustness regressions. A new destructor finding is a robustness fix, not a
  contract defect that reopens the API freeze criteria.
- Consumers that need guaranteed isolation from misbehaving destructors must
  either keep their own types' `Drop` panic-free or isolate untrusted code
  outside the process.
