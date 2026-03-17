# Changelog

All notable changes to cortex-works-minimal will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.1.0 (2026-03-17)

### Added

- Cross-platform release script with automated CHANGELOG management
- Production-grade cleanup: removal of dead code and unused structures
- Action name constants (`ACTION_STATUS`, `ACTION_ADD`) in grammar manager to prevent schema drift
- Schema integrity tests for tooling validation
- Token-efficiency documentation and agent workflow guidance
- Multi-root workspace support with prefixed path routing
- Auto-Healer for Rust syntax error repair via local LLM

### Changed

- Rewritten README with production narrative and tool tables
- Updated DEVELOPING.md with accurate build and test commands
- Added Schema Source of Truth documentation in ARCH.md

### Fixed

- Removed `ShellExecParams` unused struct from cortex-act library
- Removed `primary_root()` dead code from server.rs
- Deleted scratch files `task.md`, `test_lang.rs`, and workspace `Cargo.lock`
