# Context Reading Guide

Read this during orientation to know what context to load. Don't read everything —
read selectively based on your feature.

## Start with INDEX.md

Read `context/INDEX.md` first. It lists every entry with a one-line summary (~1 token each).
Scan it to find what's relevant before reading any full files.

## Three levels of depth

```
context/INDEX.md        → scan one-liners (~1 token/entry)
context/{category}/*.md → read distilled knowledge (~200 tokens)
references/repo/file.rs → read actual source (only when needed)
```

Always start at the cheapest level. Only drill deeper when the summary isn't enough.

## What to read (in order)

### 1. Always read
- `context/INDEX.md` — scan for relevant entries
- `feedback/session-review.md` — handoff from last session (verify results, flags, guidance)
- All of `context/gotchas/` — short warnings (<50 lines each), always worth knowing

### 2. Scope-relevant
- `context/decisions/` — entries matching your feature's scope
- `context/patterns/` — entries matching your feature's scope

How to filter: check `forge.toml` scopes → find your feature's scope → grep slugs
for related terms. Read matches only.

### 3. If feature depends on POC
- Read `context/poc/` entries for POC feature IDs in your `depends_on` list
- Check if the POC passed, failed, or pivoted — this affects your approach

### 4. Reference material (on demand)
- `context/references/` — organized by topic (e.g., `memory-management.md`, not `tigerbeetle.md`)
- Scan slug names in INDEX.md first. Read full file only if topic matches your feature.
- Each reference entry has a "Deep Dive" section with file paths into `references/`
- If the distilled summary isn't enough, read the specific source file listed there

### 5. Raw sources (last resort)
- `references/` contains cloned repos, PDFs, raw codebases (gitignored, local only)
- NEVER browse a full repo. Use `grep` or read specific files pointed to by context entries.
- This is for when you need exact API signatures, implementation details, or code to adapt.

## Rule

List directory contents first, read selectively. Never read all 50+ files when 5 are relevant.
