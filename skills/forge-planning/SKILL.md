---
name: forge-planning
description: >
  Project architect skill for forge. Detects planning phase (research, POC, full),
  scaffolds DESIGN.md, generates features.json with verify scripts. Interactive —
  discuss architecture with the user before generating features.
  Triggers: "forge planning", "plan features", "generate features".
---

# Forge Planning

You are the project architect. You do NOT write application code. You scaffold the
environment, define the roadmap, and create verify scripts.

## Phase 1: Orientation

Read these files (skip missing ones):
- `forge.toml` — project config, scopes, principles
- `DESIGN.md` — project design document
- `features.json` — existing features (if replanning)
- `context/` — all existing decisions, gotchas, patterns, poc, references

## Phase 2: Detect planning phase

Auto-detect based on project state:

### Phase 0 — Research (no DESIGN.md or empty `context/references/`)

The user has a goal but no design. Help them get there:

1. Discuss the goal, constraints, and unknowns with the user
2. **Gather reference material** — see [REFERENCES.md](REFERENCES.md) for the distillation protocol
3. Help user set up `references/` (gitignored) for raw repos, PDFs, codebases
4. Distill each source into `context/references/{topic}.md` organized by TOPIC, not by source
5. **Extract patterns from references**: For each reference, decide — is this prescriptive enough
   to be a rule? If yes, also write `context/patterns/{rule}.md`. The reference explains WHY
   (knowledge), the pattern says WHAT TO DO (rule). Example: reference `rust-compile-optimization.md`
   → pattern `cargo-dev-profile.md` ("always use mold linker, opt-level 3 for deps").
6. Scaffold `DESIGN.md` with all 8 sections (see [COVERAGE.md](COVERAGE.md))
6. Mark unknowns with `[ ]` checkboxes — things that need prototyping to answer
7. Run `forge install` to regenerate `context/INDEX.md`

All context written during research is immediately available to POC and implementation
agents — context is shared across all phases via the file system.

**Definition of Done**: DESIGN.md exists with all 8 sections. Do NOT generate features yet.
The user must review and approve the design before proceeding to Phase 1.

### Phase 1 — POC (DESIGN.md has `[ ]` unknowns)

Unknowns need prototyping before full implementation:

1. Check DESIGN.md coverage per [COVERAGE.md](COVERAGE.md), report gaps
2. For each `[ ]` unknown, generate a `"type": "poc"` feature with `p` prefix ID
3. POC features validate assumptions — their deliverable is `context/poc/{id}.md`
4. Add a `review` feature after each POC batch. The review feature must:
   - Read `context/poc/` outcomes and update DESIGN.md unknowns
   - **Promote proven approaches to patterns**: if a POC passed, convert the proven
     technique from `references/` into a concrete `patterns/` entry that all agents follow
5. POC verify scripts: run the viability test + check `context/poc/{id}.md` exists

**Definition of Done**: features.json has POC features with verify scripts. No implementation
features yet — those come after POC results resolve unknowns.

### Phase 2 — Full (no `[ ]` unknowns remain)

All unknowns resolved (`[x]` confirmed or `[!]` pivoted):

1. Check full coverage per [COVERAGE.md](COVERAGE.md) — all 8 sections complete
2. Do not proceed until the user is satisfied with coverage
3. Decompose into `implement` + `review` features with proper deps
4. Review features check scope boundaries between implementation batches

**Definition of Done**: features.json has full production features, all verify scripts written.

## Phase 3: Generate features

Feature JSON schema:
```json
{
  "id": "f001",
  "type": "implement",
  "scope": "scope-name",
  "description": "Concrete deliverable. See context/references/memory-management.md for allocator patterns.",
  "verify": "./scripts/verify/f001.sh",
  "depends_on": [],
  "priority": 1,
  "status": "pending",
  "claimed_by": null,
  "blocked_reason": null,
  "context_hints": ["references/memory-management", "decisions/use-vec"]
}
```

Feature types:
- `implement` — write code, verify with tests. Use `f` prefix IDs.
- `review` — check boundaries, update docs, verify with lint + import checks. Use `r` prefix IDs.
- `poc` — prototype to resolve unknowns. Use `p` prefix IDs.

### `context_hints` — push context, don't make agents pull

For each feature, list the context entries the agent should read. Format: `"category/slug"`.
The agent reads these during orientation — no scanning or browsing needed.

How to decide what to include:
- Which `context/references/` entries cover patterns the agent will need?
- Which `context/decisions/` explain choices that constrain this feature?
- Which `context/gotchas/` warn about pitfalls in this feature's area?
- If the feature depends on a POC, include `poc/{id}`

Also embed the most critical pointer in the `description` text itself — the agent
reads the description first and may not check `context_hints` until later.

Set `depends_on` based on data flow (what must exist before this works).
Add `review` features between implementation batches.
POC features depend only on scaffold — they should be early in the plan.

## Phase 4: Write verify scripts with principle enforcement

Write `scripts/verify/{id}.sh` for each feature. Every verify script enforces the 4 principles:

**P3 (Style) — every script includes:**
```bash
cargo fmt --check || exit 1
cargo clippy -- -D warnings || exit 1
```

**P2 (Proof) — implement features:**
- Run specific tests that prove the deliverable works, not just compilation
- Tests must follow the 7 testing rules (behavior-based, edge cases, descriptive names)

**P2 (Proof) — POC features:**
- Run the viability test for the POC
- Check outcome file exists: `test -f context/poc/{id}.md || exit 1`

**P4 (Boundaries) — review features:**
- Check scope boundary imports (no internal cross-scope access)
- Verify API surface matches design doc

## Phase 5: DESIGN.md unknowns format

Use checkboxes to track unknowns:
- `[ ]` — unresolved, needs POC or research
- `[x]` — resolved, confirmed by POC or decision
- `[!]` — pivoted, original approach failed, see `context/poc/` for details

Each unknown should reference a POC feature ID when one exists:
```markdown
[ ] Can nom parse our thrift IDL dialect? → p001
[x] SQLite handles our write volume → p002 (confirmed: 50k writes/sec)
[!] Redis pub/sub too complex for MVP → p003 (pivoted to channels)
```

## Phase 6: Validate

1. Run `forge verify` to confirm all scripts are executable and valid
2. Review the full features.json with the user
3. Confirm dependency ordering makes sense
4. Ensure every feature has a verify script that actually tests its deliverable

## Priority ordering

Lower number = higher priority. Features with unmet deps are auto-skipped.
Group by scope, order by data flow within scope.
POC features should have the lowest priority numbers (run first).
