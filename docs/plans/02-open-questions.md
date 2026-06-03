# Open Questions & Decision Log

> Track decisions made during plan review. Update this file as we iterate.

---

## Pending decisions

No blocking open questions at this time.

---

## Resolved decisions

| ID | Decision | Date | Rationale |
|----|----------|------|-----------|
| OQ-1 | Binary name `gdrive-optimize` with `gdo` alias | 2026-05-28 | Operator approved |
| OQ-2 | `drive.metadata.readonly` at login; upgrade to `drive` on first write | 2026-05-28 | Least privilege until mutation needed |
| OQ-3 | Local SQLite only in v1 | 2026-05-28 | Operator approved |
| OQ-4 | Ignore already-trashed items in v1 — do not sync or analyze trashed inventory | 2026-05-28 | Recoverable `trash` moves are supported; permanent delete and trash inventory remain deferred |
| OQ-5 | My Drive only in v1; Shared Drives in follow-on phase | 2026-05-28 | Narrower scope for initial release |
| OQ-6 | Multi-account deferred to v2; single account in v1 | 2026-05-29 | Keeps unattended v1 implementation focused |
| OQ-7 | Default report output path is `reports/<YYYY-MM-DD>/` | 2026-05-29 | Deterministic output for operators and tests |
| OQ-8 | Toolchain baseline is latest stable Rust at implementation start | 2026-05-29 | Avoids artificial MSRV friction during unattended build |
| OQ-9 | Database stack is `rusqlite` + `refinery` in v1 | 2026-05-29 | Removes migration-stack ambiguity before scaffolding |
| OQ-10 | CLI rendering stack is `anstream`, `anstyle`, `indicatif`, `comfy-table`, and `console` | 2026-05-29 | Locks terminal UX dependencies before implementation |
| OQ-11 | Token storage in v1 uses local files with `0600` permissions; OS keyring is post-v1 | 2026-05-29 | Corrects the security model and removes a false encryption assumption |
| OQ-12 | Public releases follow Semantic Versioning, starting with `v0.0.1` for the first implementation release | 2026-05-30 | Separates feature-scope labels from public release numbering |
| OQ-13 | Commits and PR titles use Conventional Commits; branch names use `<type>/<short-kebab-case>` | 2026-05-30 | Standardizes history, automation, and review hygiene |
| OQ-14 | All commits merged to the main branch must be cryptographically signed | 2026-05-30 | Establishes provenance and repository integrity requirements |

---

## Review notes

```
2026-05-28 — Operator review:
- OQ-1: ok (both names)
- OQ-2: readonly until write
- OQ-3: ok (local DB)
- OQ-4: ignore already-trashed inventory for now; recoverable `trash` moves are supported, but no permanent delete/empty-trash behavior
- OQ-5: My Drive only; Shared Drives as follow-on phase
```
