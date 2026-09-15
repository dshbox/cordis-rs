# Coding standards

Standing cross-cutting rules for code review. Domain rules live in
`CONTEXT.md` and `docs/adr/` — this file
carries only the cross-cutting checks that reviews kept finding the
hard way, one precedent each. Documented rules here are citable as
hard violations; tooling-enforced checks stay in CI (`ci/gates.sh`).

## Comments match the code they sit in

A comment that narrates an ordering, ownership, or lifetime the final
code does not have is a finding — comment and code must agree,
whichever of the two changes.

Precedent: ticket 26's review caught a "clone before the lock" comment
whose clone ran under the guard (1bfab7c).

## Prose counts match what they count

Where a README, module doc, or tour header names a count — a number of
beats, sections, or items, a "trio" — the artifact contains exactly
that many. Count drift is a finding.

Precedent: ticket 22's review fixed a README that folded `teardown`
into a trio (e598182); ticket 26's fixed "Four beats" over five
sections (1bfab7c).

## Promotion evidence is discriminating

A promotion mark or parity row names its scenario, demonstrates it
exactly as named, and uses evidence that would fail under the nearest
rival explanation. Evidence both explanations satisfy proves nothing.

Precedent: ticket 25's review rejected an "in a loop" promotion
demonstrated unrolled, and an intern-reuse claim that realm GC would
satisfy identically (1163711).
