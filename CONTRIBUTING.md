# Contributing

## Release Policy

- Public releases follow [Semantic Versioning 2.0.0](https://semver.org/).
- The first implementation release targeted by the current plan is `v0.0.1`.
- Tags use the `vX.Y.Z` format.

## Git Workflow

- Commits must follow the Conventional Commits format, for example `feat(sync): add bootstrap replay`.
- Pull request titles must use the same Conventional Commits header format.
- Branch names must use `<type>/<short-kebab-case>`, for example `feat/bootstrap-sync`.
- Commits merged to the main branch must be cryptographically signed.

## Phase 0 Notes

- The workspace is scaffolded for implementation.
- Command help pages are considered part of the public interface.
- Shared fixtures live under `tests/fixtures/`; package-local integration and functional tests live with the owning crate.
