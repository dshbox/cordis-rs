# Changelog

## [Unreleased]

## [0.3.28](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.3.27...cordis-loader-v0.3.28) - 2026-09-29

### Fixed

- *(loader)* reject unknown source-schema fields

### Changed

- *(loader)* **Breaking `Deserialize` behaviour:** the source schema
  (`PluginEntry`, `EntryGroup`, `InjectEntry`, `IsolateEntry`, `RealmPolicy`)
  now rejects unknown fields, including fields that belong to another tagged
  variant (for example `label` on `private`, `config` on `required`). A
  misspelled `disabled` or `isolate` previously parsed with the default and
  executed the Plugin or placed its Services in the caller's realm; it is now a
  parse error. `Serialize` output is unchanged.

## [0.3.23](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.3.22...cordis-loader-v0.3.23) - 2026-09-26

### Fixed

- *(loader)* route rollback destructor diagnostics to logger
- *(loader)* contain rollback member input destruction

## [0.3.14](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.3.13...cordis-loader-v0.3.14) - 2026-09-25

### Fixed

- *(core)* pin committed apply work to completion runtime

## [0.3.4](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.3.3...cordis-loader-v0.3.4) - 2026-09-16

### Fixed

- harden CI, release, and review contracts ([#138](https://github.com/dshbox/cordis-rs/pull/138))

## [0.3.3](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.3.2...cordis-loader-v0.3.3) - 2026-09-16

### Other

- tighten release docs and ci policy ([#136](https://github.com/dshbox/cordis-rs/pull/136))

## [0.3.2](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.3.1...cordis-loader-v0.3.2) - 2026-09-16

### Fixed

- harden shutdown and timer contracts ([#133](https://github.com/dshbox/cordis-rs/pull/133))

## [0.2.1](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.2.0...cordis-loader-v0.2.1) - 2026-09-15

### Other

- update readmes for 0.8 release line ([#92](https://github.com/dshbox/cordis-rs/pull/92))

## [0.2.0](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.1.3...cordis-loader-v0.2.0) - 2026-09-15

### Other

- [**breaking**] rename Fork to FiberHandle ([#90](https://github.com/dshbox/cordis-rs/pull/90))

### Changed

- [**breaking**] align Loader outcomes with `FiberHandle`: `Spawned::fiber_handle` replaces `fork` and `LoadOutcome::fiber_handles()` replaces `forks()`

## [0.1.1](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.1.0...cordis-loader-v0.1.1) - 2026-09-14

### Fixed

- address post-v3 review findings ([#74](https://github.com/dshbox/cordis-rs/pull/74))

## 0.1.0

- First Cordis v3 `cordis-loader` release line.
