# CLI UX

## Purpose

The command line is a primary product surface. Phase 0 records the intended operator experience before implementation starts.

```mermaid
flowchart LR
    HELP[--help] --> DISCOVER[discoverability]
    PROGRESS[reactive progress] --> CONFIDENCE[operator confidence]
    PREVIEW[dry-run previews] --> SAFETY[safe mutations]
    JSON[json/md output] --> AUTOMATION[scriptability]
```

## UX baseline

- RGB output is used on capable terminals with textual fallbacks.
- Every command and subcommand must provide examples in help text.
- Mutations preview by default and require explicit apply semantics.
- Non-interactive mode suppresses redraw-based UI.
- Width-sensitive table rendering is preferred over hard-coded layouts.

## Selected libraries

- `anstream` + `anstyle`
- `indicatif`
- `comfy-table`
- `console`
