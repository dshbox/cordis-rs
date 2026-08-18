# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.12](https://github.com/dshbox/cordis-rs/compare/cordis-include-v0.0.11...cordis-include-v0.0.12) - 2026-08-18

### Other

- add Chinese READMEs for the satellite crates
- align loader, CLI, and include docs with actual behavior

## [0.0.11](https://github.com/dshbox/cordis-rs/compare/cordis-include-v0.0.10...cordis-include-v0.0.11) - 2026-08-18

### Fixed

- serialize LoaderFile writes so concurrent writers cannot tear the file

## [0.0.10](https://github.com/dshbox/cordis-rs/compare/cordis-include-v0.0.9...cordis-include-v0.0.10) - 2026-08-18

### Other

- updated the following local packages: cordis-rs

## [0.0.9](https://github.com/dshbox/cordis-rs/compare/cordis-include-v0.0.8...cordis-include-v0.0.9) - 2026-08-17

### Fixed

- correct loader reload, imports, inject, and dispose semantics

### Other

- Merge pull request #22 from dshbox/fix/audit-major

## [0.0.8](https://github.com/dshbox/cordis-rs/compare/cordis-include-v0.0.7...cordis-include-v0.0.8) - 2026-08-17

### Other

- updated the following local packages: cordis-rs

## [0.0.7](https://github.com/dshbox/cordis-rs/compare/cordis-include-v0.0.6...cordis-include-v0.0.7) - 2026-08-17

### Other

- Merge pull request #18 from dshbox/docs/readme-refresh
- align READMEs with the current workspace and feature set

## [0.0.6](https://github.com/dshbox/cordis-rs/compare/cordis-include-v0.0.5...cordis-include-v0.0.6) - 2026-08-17

### Added

- coalesced deferred writes on LoaderFile

### Other

- Merge pull request #16 from dshbox/feat/loader-events-debounce

## [0.0.5](https://github.com/dshbox/cordis-rs/compare/cordis-include-v0.0.4...cordis-include-v0.0.5) - 2026-08-17

### Added

- import markers and removed-entry paths

## [0.0.4](https://github.com/dshbox/cordis-rs/compare/cordis-include-v0.0.3...cordis-include-v0.0.4) - 2026-08-17

### Other

- add crate READMEs and an ecosystem overview

## [0.0.3](https://github.com/dshbox/cordis-rs/compare/cordis-include-v0.0.2...cordis-include-v0.0.3) - 2026-08-17

### Added

- add EntryOptions::with_inject builder

### Fixed

- allow update_entry to keep the entry's own id

## [0.0.2](https://github.com/dshbox/cordis-rs/compare/cordis-include-v0.0.1...cordis-include-v0.0.2) - 2026-08-17

### Added

- add cordis-include for config entry trees and loader files

### Fixed

- scope watch matching to the target file and isolate watch tests
- match watcher events through symlinked paths
- satisfy stable clippy and macOS FSEvents in cordis-include
