# Sync Engine

## Purpose

This document locks the crash-safety model before implementation. Phase 0 does not implement sync behavior, but it fixes the bootstrap and delta flow that later phases must preserve.

```mermaid
sequenceDiagram
    participant CLI
    participant API as Drive API
    participant DB as SQLite

    CLI->>API: getStartPageToken()
    API-->>CLI: checkpoint token
    CLI->>DB: create staging area
    CLI->>API: files.list(...)
    API-->>CLI: metadata pages
    CLI->>DB: write snapshot pages
    CLI->>API: changes.list(...)
    API-->>CLI: drift since checkpoint
    CLI->>DB: validate and atomically swap
```

## Invariants

- Committed sync tokens advance only after DB changes commit.
- Reports read the last committed snapshot only.
- Full rebuilds use staging before swap.
- Delta pages are restart-safe from the last committed token.

## Phase 1 handoff

Phase 1 will add repository migrations, journaling, and `changes.list` replay on top of this contract.
