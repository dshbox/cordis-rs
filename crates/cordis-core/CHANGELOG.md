# Changelog

## [Unreleased]

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
