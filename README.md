# HandShaker Protocol

HandShaker is Smartisan's discontinued Android file-transfer and device-management tool. This repository preserves a clean foundation for independently documenting its communication protocol and building a compatible, cross-platform Rust backend.

## Goals

1. Derive and document the complete HandShaker communication protocol from the locally retained reverse-engineering artifacts.
2. Build a modern, reusable Rust backend that can interoperate with the original HandShaker clients and supports future GUI applications.

## Platform priority

1. Modern ARM64 macOS
2. Linux
3. Other platforms

## Repository layout

- `docs/` — protocol documentation, research notes, and design records.
- Local reverse-engineering artifacts are intentionally excluded from Git. They remain available in the working tree as source material and are not redistributed by this repository.

## Current status

The repository is intentionally at the initialization stage. No protocol conclusions or implementation code have been added yet.
