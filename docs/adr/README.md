# ADR index and historical labels

The public, normative Cordis v3 ADR set is **0028 through 0039**. Read it
through [`docs/v3-architecture.md`](../v3-architecture.md#decision-index),
which is the architecture entry point and owns the current decision index.

Source comments may also mention **ADR 0001 through ADR 0027**. Those numbers
are historical provenance labels from the pre-promotion v3 research repository;
they are not part of this public repository's normative ADR set and their files
are intentionally not reproduced here. A historical reference in an
implementation comment therefore explains lineage only—it never overrides the
current architecture, public-interface inventory, or accepted ADRs 0028–0039.

Likewise, research revisions such as `2dda99f3d913a1410d1263aed8bd75d4e4dfb6f1`
and `592cb4af0e36b9c0d9df0265462c1cf4103ff10c` are evidence identifiers from
that pre-promotion repository, not object IDs guaranteed to resolve in the
public Git history. [`docs/v3-migration.md`](../v3-migration.md) records that
historical evidence explicitly and is non-normative.
