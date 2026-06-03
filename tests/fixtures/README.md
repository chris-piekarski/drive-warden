# Fixture datasets

Fixture datasets are the offline contract for functional and acceptance tests.

## Dataset roots

- `drive_small/` contains the main happy-path mock dataset. It covers duplicates, sharing findings, stale/large files, path edge cases, and an image item for `inspect exif`.
- `drive_duplicates/` is reserved for expanded MD5 and heuristic duplicate coverage.
- `drive_sharing/` is reserved for public, domain, direct, and inherited permission coverage beyond the shared baseline fixture.
- `drive_paths/` is reserved for nested, orphaned, shortcut, and multi-parent path cases beyond the shared baseline fixture.
- `drive_failures/` contains scenario-specific mock datasets for revoked token, invalid token, and interrupted sync recovery flows.
- `drive_reports/` is reserved for golden Markdown report outputs.

## Shared layout

Each dataset root contains the same scaffolded directories:

```text
tests/fixtures/<dataset>/
├── api/
└── expected/
    ├── cli/
    ├── db/
    └── reports/
```

Use `tests/config/mock.toml` for the default happy-path dataset, or point a temp config at a specific `drive_failures/<scenario>/` directory when testing alternative flows.
