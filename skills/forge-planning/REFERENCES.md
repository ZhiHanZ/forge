# Reference Distillation Protocol

How to turn large reference sources (repos, papers, docs) into agent-consumable context.

## Three-level progressive disclosure

```
context/INDEX.md        → ~1 token/entry  (scan to find what exists)
context/references/*.md → ~200 tokens     (read distilled knowledge)
references/repo/file.rs → full source     (read when you need details)
```

Agents start at the cheapest level and drill down only when needed.

## Setup `references/` directory

The `references/` directory (gitignored) holds raw material:

```
references/
├── doris/              # git clone --depth 1 --branch 4.0.1
├── tigerbeetle/        # git clone --depth 1
├── volo/               # git clone --depth 1
├── slatedb/            # git clone --depth 1
├── bf-tree/            # git clone --depth 1
└── papers/
    ├── bf-tree-vldb2024.pdf
    └── other-paper.pdf
```

Clone with `--depth 1` to minimize disk. This directory is local-only (gitignored).

## Distill by TOPIC, not by source

**Wrong**: `context/references/tigerbeetle.md`, `context/references/volo.md`
**Right**: `context/references/memory-management.md`, `context/references/rpc-patterns.md`

Topics combine insights from multiple sources. A coding agent working on memory
management reads ONE file that covers BF-Tree allocator + TigerBeetle static
allocation + SlateDB page caching — instead of reading three separate files.

## Reference entry structure

Each `context/references/{topic}.md` has:

```markdown
---
source: [primary URL]
tags: [topic, subtopic]
---

# {Topic Name}

## Key Patterns
- Concrete code snippets and API signatures
- Design decisions and WHY they were made

## Deep Dive
For detailed source, see:
- `references/tigerbeetle/src/lsm/manifest.zig` — manifest compaction
- `references/bf-tree/src/allocator.rs` — circular buffer allocator
- `references/doris/fe/src/main/java/...` — query planner entry point
```

The "Deep Dive" section gives agents file paths into `references/` for when
the distilled summary isn't enough. They read these only when needed.

## Distillation rules

1. **~200 tokens per entry** — working knowledge, not a textbook
2. **Code > prose** — show the actual function signature, not "it has a function that..."
3. **Include WHY** — "static allocation because no GC pauses" not just "static allocation"
4. **File paths for depth** — point to specific files in `references/`, not whole repos
5. **Cross-reference entries** — "see also: context/references/concurrency.md"
6. **Skip what Claude already knows** — don't explain what a B-tree is

## After distillation

Run `forge install` to regenerate `context/INDEX.md`. This gives agents a
scannable table of contents for all context entries.
