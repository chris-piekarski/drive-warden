# Fixtures

## Purpose

Fixtures are the primary offline verification surface for the project. The shipped Phase 4 suites use them to cover happy-path sync, EXIF inspection, sharing cleanup, and failure-mode recovery without any live Google dependency.

## Dataset families

- `drive_small`
- `drive_duplicates`
- `drive_sharing`
- `drive_paths`
- `drive_failures`
- `drive_reports`

## Shared layout

```text
tests/fixtures/<dataset>/
├── api/
├── expected/
│   ├── db/
│   ├── cli/
│   └── reports/
```

## Active datasets

- `drive_small`
  Primary happy-path mock account dataset used by functional and acceptance tests. Includes duplicates, sharing findings, path edge cases, and an image file with EXIF-compatible metadata.
- `drive_failures/revoked_token`
  Simulates a revoked or expired session so the CLI can surface re-login guidance.
- `drive_failures/invalid_page_token`
  Simulates a `changes.list` invalid-token failure and verifies `sync --full` guidance.
- `drive_failures/interrupted_sync`
  Simulates an interrupted delta sync and verifies the prior committed snapshot remains intact.

## Validation

Run `make fixtures-validate` to verify the required dataset roots and shared directory layout are present.
