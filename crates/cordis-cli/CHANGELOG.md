# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.20](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.19...cordis-cli-v0.0.20) - 2026-08-19

### Other

- updated the following local packages: cordis-rs, cordis-loader

## [0.0.19](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.18...cordis-cli-v0.0.19) - 2026-08-19

### Other

- updated the following local packages: cordis-rs, cordis-loader

## [0.0.18](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.17...cordis-cli-v0.0.18) - 2026-08-18

### Other

- updated the following local packages: cordis-rs, cordis-loader

## [0.0.17](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.16...cordis-cli-v0.0.17) - 2026-08-18

### Fixed

- stop the lib/bin rustdoc output collision between cordis-rs and cordis-cli ([#59](https://github.com/dshbox/cordis-rs/pull/59))

## [0.0.16](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.15...cordis-cli-v0.0.16) - 2026-08-18

### Other

- updated the following local packages: cordis-loader

## [0.0.15](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.14...cordis-cli-v0.0.15) - 2026-08-18

### Other

- updated the following local packages: cordis-loader

## [0.0.14](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.13...cordis-cli-v0.0.14) - 2026-08-18

### Other

- updated the following local packages: cordis-loader

## [0.0.13](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.12...cordis-cli-v0.0.13) - 2026-08-18

### Other

- updated the following local packages: cordis-rs, cordis-loader

## [0.0.12](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.11...cordis-cli-v0.0.12) - 2026-08-18

### Other

- updated the following local packages: cordis-rs, cordis-loader

## [0.0.11](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.10...cordis-cli-v0.0.11) - 2026-08-18

### Other

- updated the following local packages: cordis-rs, cordis-loader

## [0.0.10](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.9...cordis-cli-v0.0.10) - 2026-08-18

### Other

- add Chinese READMEs for the satellite crates
- align loader, CLI, and include docs with actual behavior

## [0.0.9](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.8...cordis-cli-v0.0.9) - 2026-08-18

### Fixed

- rebuild OsStrings from argument bytes without windows-only APIs
- forward daemon shutdown to the worker and bound its teardown
- [**breaking**] keep non-UTF-8 CLI arguments intact through argument parsing

## [0.0.8](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.7...cordis-cli-v0.0.8) - 2026-08-18

### Other

- updated the following local packages: cordis-rs, cordis-loader

## [0.0.7](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.6...cordis-cli-v0.0.7) - 2026-08-17

### Fixed

- propagate cordis run boot failures and restart with backoff

### Other

- Merge pull request #22 from dshbox/fix/audit-major
- pick the freshest rlib in the fixture build harnesses

## [0.0.6](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.5...cordis-cli-v0.0.6) - 2026-08-17

### Added

- hot-restart workers when plugin libraries change

### Other

- also search registry windows_* lib dirs for fixture linking
- build dynamic fixtures portably on windows

## [0.0.5](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.4...cordis-cli-v0.0.5) - 2026-08-17

### Other

- Merge pull request #18 from dshbox/docs/readme-refresh
- align READMEs with the current workspace and feature set

## [0.0.4](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.3...cordis-cli-v0.0.4) - 2026-08-17

### Other

- updated the following local packages: cordis-loader

## [0.0.3](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.2...cordis-cli-v0.0.3) - 2026-08-17

### Other

- updated the following local packages: cordis-loader

## [0.0.2](https://github.com/dshbox/cordis-rs/compare/cordis-cli-v0.0.1...cordis-cli-v0.0.2) - 2026-08-17

### Added

- add cordis-cli crate
