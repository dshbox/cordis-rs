# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
