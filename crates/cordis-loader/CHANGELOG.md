# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
