# Changelog

## [Unreleased]

## [0.3.16](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.15...cordis-core-v0.3.16) - 2026-09-25

### Fixed

- *(core)* finish era dependent barrier after input Drop panic

## [0.3.15](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.14...cordis-core-v0.3.15) - 2026-09-25

### Fixed

- *(core)* keep committed update owned across input Drop panic

## [0.3.14](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.13...cordis-core-v0.3.14) - 2026-09-25

### Fixed

- *(core)* pin committed apply work to completion runtime

### Other

- *(core)* await the restart contract, not its scheduler

## [0.3.13](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.12...cordis-core-v0.3.13) - 2026-09-24

### Other

- *(core)* accept completed convergence in old-wake ready probe

## [0.3.12](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.11...cordis-core-v0.3.12) - 2026-09-24

### Fixed

- *(core)* preserve cleanup barriers and dependent convergence across handoff ([#164](https://github.com/dshbox/cordis-rs/pull/164))

## [0.3.11](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.10...cordis-core-v0.3.11) - 2026-09-24

### Fixed

- *(core)* include late source service edges in era swap preflight ([#162](https://github.com/dshbox/cordis-rs/pull/162))

## [0.3.10](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.9...cordis-core-v0.3.10) - 2026-09-24

### Fixed

- fix v3 service mutation and ready lifecycle races ([#160](https://github.com/dshbox/cordis-rs/pull/160))

## [0.3.9](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.8...cordis-core-v0.3.9) - 2026-09-19

### Other

- close operational failure maturity audit ([#156](https://github.com/dshbox/cordis-rs/pull/156))

## [0.3.8](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.7...cordis-core-v0.3.8) - 2026-09-19

### Other

- *(core)* deepen lifecycle correctness assurance ([#151](https://github.com/dshbox/cordis-rs/pull/151))

## [0.3.7](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.6...cordis-core-v0.3.7) - 2026-09-18

### Other

- define 1.0 compatibility policy ([#146](https://github.com/dshbox/cordis-rs/pull/146))

## [0.3.6](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.5...cordis-core-v0.3.6) - 2026-09-16

### Other

- *(core)* replace watchdog polling with wakeup ([#143](https://github.com/dshbox/cordis-rs/pull/143))

## [0.3.5](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.4...cordis-core-v0.3.5) - 2026-09-16

### Fixed

- *(core)* assert event hook order invariant ([#141](https://github.com/dshbox/cordis-rs/pull/141))

## [0.3.4](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.3...cordis-core-v0.3.4) - 2026-09-16

### Fixed

- harden CI, release, and review contracts ([#138](https://github.com/dshbox/cordis-rs/pull/138))

## [0.3.3](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.2...cordis-core-v0.3.3) - 2026-09-16

### Other

- tighten release docs and ci policy ([#136](https://github.com/dshbox/cordis-rs/pull/136))

## [0.3.2](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.1...cordis-core-v0.3.2) - 2026-09-16

### Fixed

- harden shutdown and timer contracts ([#133](https://github.com/dshbox/cordis-rs/pull/133))

### Other

- *(core)* store removal release permit ([#135](https://github.com/dshbox/cordis-rs/pull/135))

## [0.3.1](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.3.0...cordis-core-v0.3.1) - 2026-09-15

### Other

- *(core)* establish performance regression baseline ([#131](https://github.com/dshbox/cordis-rs/pull/131))

## [0.3.0](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.2.12...cordis-core-v0.3.0) - 2026-09-15

### Fixed

- *(core)* remove unreachable lifecycle errors ([#127](https://github.com/dshbox/cordis-rs/pull/127))

## [0.2.12](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.2.11...cordis-core-v0.2.12) - 2026-09-15

### Fixed

- *(core)* carry lifecycle attribution across task spawn ([#125](https://github.com/dshbox/cordis-rs/pull/125))

## [0.2.11](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.2.10...cordis-core-v0.2.11) - 2026-09-15

### Fixed

- *(core)* release wait-state watchdog threads ([#123](https://github.com/dshbox/cordis-rs/pull/123))

## [0.2.10](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.2.9...cordis-core-v0.2.10) - 2026-09-15

### Other

- *(core)* close era convergence contracts ([#119](https://github.com/dshbox/cordis-rs/pull/119))

## [0.2.9](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.2.8...cordis-core-v0.2.9) - 2026-09-15

### Other

- *(core)* model era handoff ownership ([#117](https://github.com/dshbox/cordis-rs/pull/117))

## [0.2.8](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.2.7...cordis-core-v0.2.8) - 2026-09-15

### Other

- *(core)* model era source arbitration ([#115](https://github.com/dshbox/cordis-rs/pull/115))

## [0.2.7](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.2.6...cordis-core-v0.2.7) - 2026-09-15

### Other

- *(core)* close notify retry contracts ([#113](https://github.com/dshbox/cordis-rs/pull/113))

## [0.2.6](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.2.5...cordis-core-v0.2.6) - 2026-09-15

### Other

- *(core)* pin inertia notify contracts ([#111](https://github.com/dshbox/cordis-rs/pull/111))

## [0.2.5](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.2.4...cordis-core-v0.2.5) - 2026-09-15

### Other

- *(core)* close phase one convergence models ([#109](https://github.com/dshbox/cordis-rs/pull/109))

## [0.2.4](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.2.3...cordis-core-v0.2.4) - 2026-09-15

### Other

- *(core)* model ready linearization ([#107](https://github.com/dshbox/cordis-rs/pull/107))

## [0.2.3](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.2.2...cordis-core-v0.2.3) - 2026-09-15

### Fixed

- *(core)* commit service drift with visibility ([#102](https://github.com/dshbox/cordis-rs/pull/102))

### Other

- *(core)* model releasing kick authority ([#106](https://github.com/dshbox/cordis-rs/pull/106))
- *(core)* model convergence invariants with loom ([#104](https://github.com/dshbox/cordis-rs/pull/104))

## [0.2.2](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.2.1...cordis-core-v0.2.2) - 2026-09-15

### Fixed

- *(core)* report detached framework panics ([#97](https://github.com/dshbox/cordis-rs/pull/97))

## [0.2.1](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.2.0...cordis-core-v0.2.1) - 2026-09-15

### Other

- update readmes for 0.8 release line ([#92](https://github.com/dshbox/cordis-rs/pull/92))

## [0.2.0](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.1.3...cordis-core-v0.2.0) - 2026-09-15

### Other

- [**breaking**] rename Fork to FiberHandle ([#90](https://github.com/dshbox/cordis-rs/pull/90))

### Changed

- [**breaking**] rename the consumer lifecycle control type from `Fork` to `FiberHandle`; no compatibility alias is retained

## [0.1.3](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.1.2...cordis-core-v0.1.3) - 2026-09-14

### Fixed

- *(events)* index listener claims ([#88](https://github.com/dshbox/cordis-rs/pull/88))

## [0.1.2](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.1.1...cordis-core-v0.1.2) - 2026-09-14

### Fixed

- *(timer)* move admission probe behind internal seam ([#79](https://github.com/dshbox/cordis-rs/pull/79))

### Other

- replace scheduling spins with explicit signals ([#80](https://github.com/dshbox/cordis-rs/pull/80))

## [0.1.1](https://github.com/dshbox/cordis-rs/compare/cordis-core-v0.1.0...cordis-core-v0.1.1) - 2026-09-14

### Fixed

- *(core)* stage internal admission seam ([#77](https://github.com/dshbox/cordis-rs/pull/77))
- address post-v3 review findings ([#74](https://github.com/dshbox/cordis-rs/pull/74))

## 0.1.0

- First Cordis v3 `cordis-core` release line.
