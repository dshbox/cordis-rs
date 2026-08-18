# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.19](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.18...cordis-loader-v0.0.19) - 2026-08-18

### Other

- align READMEs with the W1-W3 assembly-layer features ([#60](https://github.com/dshbox/cordis-rs/pull/60))

## [0.0.18](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.17...cordis-loader-v0.0.18) - 2026-08-18

### Added

- [**breaking**] add the !!js expression evaluator and the disabled expression slot

## [0.0.17](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.16...cordis-loader-v0.0.17) - 2026-08-18

### Other

- updated the following local packages: cordis-include

## [0.0.16](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.15...cordis-loader-v0.0.16) - 2026-08-18

### Added

- add loader document sources (with_document, update)

### Other

- Merge pull request #53 from dshbox/feat/loader-update

## [0.0.15](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.14...cordis-loader-v0.0.15) - 2026-08-18

### Other

- updated the following local packages: cordis-include

## [0.0.14](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.13...cordis-loader-v0.0.14) - 2026-08-18

### Other

- updated the following local packages: cordis-rs, cordis-group, cordis-include

## [0.0.13](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.12...cordis-loader-v0.0.13) - 2026-08-18

### Other

- updated the following local packages: cordis-rs, cordis-group, cordis-include

## [0.0.12](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.11...cordis-loader-v0.0.12) - 2026-08-18

### Other

- updated the following local packages: cordis-rs, cordis-group, cordis-include

## [0.0.11](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.10...cordis-loader-v0.0.11) - 2026-08-18

### Other

- add Chinese READMEs for the satellite crates
- align loader, CLI, and include docs with actual behavior

## [0.0.10](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.9...cordis-loader-v0.0.10) - 2026-08-18

### Fixed

- serialize loader state transitions and repair patch/start/self-kill races

## [0.0.9](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.8...cordis-loader-v0.0.9) - 2026-08-18

### Fixed

- *(loader)* preserve state on failed reload

### Fixed

- preserve the current plugin tree when the main config cannot be read during reload

## [0.0.8](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.7...cordis-loader-v0.0.8) - 2026-08-17

### Fixed

- correct loader reload, imports, inject, and dispose semantics

### Other

- Merge pull request #22 from dshbox/fix/audit-major
- pick the freshest rlib in the fixture build harnesses

## [0.0.7](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.6...cordis-loader-v0.0.7) - 2026-08-17

### Added

- resolve plugins from dynamic libraries behind the dynamic feature

### Other

- also search registry windows_* lib dirs for fixture linking
- build dynamic fixtures portably on windows

## [0.0.6](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.5...cordis-loader-v0.0.6) - 2026-08-17

### Other

- updated the following local packages: cordis-rs, cordis-include, cordis-group

## [0.0.5](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.4...cordis-loader-v0.0.5) - 2026-08-17

### Added

- loader event family and debounced write-backs

### Other

- Merge pull request #16 from dshbox/feat/loader-events-debounce
- drop a stray trailing blank line

## [0.0.4](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.3...cordis-loader-v0.0.4) - 2026-08-17

### Added

- mount import files as entry subtrees

## [0.0.3](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.2...cordis-loader-v0.0.3) - 2026-08-17

### Other

- add crate READMEs and an ecosystem overview

## [0.0.2](https://github.com/dshbox/cordis-rs/compare/cordis-loader-v0.0.1...cordis-loader-v0.0.2) - 2026-08-17

### Added

- add cordis-loader crate
