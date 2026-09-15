# Performance regression evidence

Status: active benchmark baseline for Cordis 1.0 readiness

This document owns the performance-regression measurement contract referenced by
[`ROADMAP.md`](../ROADMAP.md). It is evidence and review policy, not normative
Runtime architecture and not a throughput promise. Runtime semantics remain owned by
the v3 architecture, public-interface inventory, and accepted ADRs.

## What the benchmark measures

The benchmark suite lives at
`crates/cordis-core/benches/runtime_paths.rs` and drives only supported public
Cordis APIs. Plugin and listener bodies are deliberately no-op or all-miss so the
measured region is dominated by Cordis protocol work rather than arbitrary consumer
work.

The fan-out size `N` is 32. The initial suite covers:

| Benchmark | Measured region | Reset/setup outside the measured region |
| --- | --- | --- |
| `lifecycle/spawn_initial_settle` | one no-op Plugin spawn through live quiescent handoff | Plugin sealing before the timer; Fiber disposal after the timer |
| `lifecycle/ready_already_quiescent` | `ready()` on an already quiescent Active Fiber | one initial spawn and final disposal |
| `event/emit/{0,1,32}` | ordered emit through 0, 1, or 32 no-op observers | listener registration |
| `event/query_all_miss/{0,1,32}` | query through 0, 1, or 32 responders that all return Miss | listener registration |
| `service/publish_and_converge/{0,1,32}` | missing-to-visible Service publication plus convergence of every affected Fiber to Active | dependent Fiber creation; publication removal and convergence back to Pending |
| `lifecycle/restart` | one same-Fiber restart with a no-op apply body | initial spawn and final disposal |
| `lifecycle/update` | one committed same-Fiber typed update with a no-op apply body | initial spawn and final disposal |
| `lifecycle/era_swap` | one successful identity-breaking era replacement through successor handoff | initial spawn and final successor disposal |

Service publication is intentionally measured through convergence rather than only
the synchronous `provide()` call. That keeps background settle work from leaking
across iterations and makes the result a complete public-path latency.

## Harness and runtime

Criterion 0.8.2 is a dev-only implementation choice. The suite uses one Tokio
multi-thread runtime with two worker threads per benchmark function. Criterion is
configured for:

- 1 second warm-up;
- 2 seconds measurement time;
- 30 samples; and
- a 15% noise threshold.

Run the suite with:

```text
cargo bench -p cordis-core --bench runtime_paths -- --noplot
```

For a same-machine before/after comparison, save the baseline from the exact
reference commit and compare the candidate against it:

```text
cargo bench -p cordis-core --bench runtime_paths -- --noplot --save-baseline <name>
cargo bench -p cordis-core --bench runtime_paths -- --noplot --baseline <name>
```

Absolute timings from different machines, power states, kernels, or Rust toolchains
must not be compared as if they were one continuous series.

## Regression review policy

The initial noise study used three back-to-back complete runs on the same host. The
median three-run span across the 14 benchmark cases was about 10.9%. Some short
paths were noisier: the observed spans reached 24.7% for zero-listener emit and
26.8% for initial spawn; one-dependent Service convergence reached 19.9%.

For that reason, benchmark output is **review evidence, not an automatic CI gate**.
The review policy is:

1. Compare only on the same reference environment and toolchain.
2. Treat a statistically significant slowdown whose point estimate exceeds 15% as
   a review trigger, not as a defect verdict.
3. Re-run the affected benchmark on the same machine. A single crossing that does
   not reproduce is classified as measurement noise and should not block a change.
4. A repeated same-direction slowdown above 15% requires human review and an
   explanation of whether the change is expected, acceptable, or a regression to
   fix before merge.
5. Correctness evidence always wins over performance evidence; no benchmark result
   justifies weakening a semantic contract.

The 15% trigger is deliberately above the suite's median observed drift. It is not a
claim that every benchmark is stable within 15%; the spawn and zero-listener emit
cases are explicit counterexamples and therefore demonstrate why every crossing is
confirmed before any conclusion.

## Initial local reference

The first recorded reference was captured on 2026-09-16 from runtime commit
`4e85a5ba722259afe22223fdf28414644c38da22`; the benchmark branch changes no production runtime code.
Environment:

- Rust `1.98.1` (`48a229cea`, LLVM 22.1.8);
- Cargo `1.98.1`;
- `x86_64-unknown-linux-gnu`, Linux `7.1.9-arch1-2`;
- 12th Gen Intel Core i9-12900HK, 20 logical CPUs visible; and
- the benchmark's two-worker Tokio runtime.

The value below is the median of the three run medians. The span is
`(max(run median) - min(run median)) / median-of-medians`, included to make the
observed local noise visible rather than hiding it behind one headline number.

| Benchmark | Initial reference | Three-run span |
| --- | ---: | ---: |
| `lifecycle/spawn_initial_settle` | 1.414 µs | 26.8% |
| `lifecycle/ready_already_quiescent` | 63.592 ns | 14.6% |
| `event/emit/0` | 50.914 ns | 24.7% |
| `event/emit/1` | 218.759 ns | 9.2% |
| `event/emit/32` | 4.505 µs | 9.9% |
| `event/query_all_miss/0` | 49.474 ns | 8.6% |
| `event/query_all_miss/1` | 218.181 ns | 6.5% |
| `event/query_all_miss/32` | 4.325 µs | 13.9% |
| `service/publish_and_converge/0` | 288.265 ns | 14.0% |
| `service/publish_and_converge/1` | 7.455 µs | 19.9% |
| `service/publish_and_converge/32` | 69.230 µs | 11.9% |
| `lifecycle/restart` | 5.005 µs | 8.1% |
| `lifecycle/update` | 5.394 µs | 5.1% |
| `lifecycle/era_swap` | 7.699 µs | 5.9% |

These numbers are a historical local reference, not a cross-machine target and not
proof that Cordis is production ready. Future changes should compare against a fresh
saved baseline on the same reference machine; this table records the first scale and
noise profile so later benchmark redesigns can also explain discontinuities.
