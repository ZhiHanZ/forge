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

### Review features are milestone gates

Review features (`type: "review"`) verify that a set of implementation features
collectively delivers a capability. They are the only mechanism for milestone enforcement.
Apply these rules strictly — a broken milestone gate means false completion:

1. **Numbered requirements in description**: The description must list explicit, testable
   requirements — not prose. Each requirement is a statement that can be checked by the
   verify script or a dependent feature.

2. **Every requirement traces to a feature**: Each requirement must be delivered by at least
   one implementation feature. If a requirement has no delivering feature, create the feature
   or drop the requirement. No orphan requirements.

3. **`depends_on` must be complete**: The review feature MUST depend on ALL features that
   deliver its requirements. A milestone that doesn't depend on its condition features can
   be claimed and marked "done" while conditions are unmet — this is the #1 cause of false
   milestone completion.

4. **Verify script tests the actual gate**: The verify script must check the milestone's
   integration requirements, not just run `cargo build/test/fmt/clippy`. If the milestone
   says "system serves queries end-to-end", the verify script must test that path.

Bad example — milestone passes without delivering its promise:
```json
{
  "id": "r001",
  "description": "M1: system serves queries end-to-end",
  "depends_on": ["f001", "f002"],
  "verify": "./scripts/verify/r001.sh"
}
```
(verify script: `cargo build && cargo test && cargo fmt --check` — never tests end-to-end)

Good example — milestone enforces what it claims:
```json
{
  "id": "r001",
  "description": "M1 milestone:\n1. Binary starts and accepts connections on port 9030\n2. SELECT 1 returns correct result through our FE\n3. Docker compose includes our service container\n4. cargo fmt + clippy clean",
  "depends_on": ["f001", "f002", "f003", "f004"],
  "verify": "./scripts/verify/r001.sh"
}
```
(verify script tests each numbered requirement; depends_on includes ALL delivery features)

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

**P2 (Proof) — review features (milestones) — CRITICAL:**
- Verify script must test EVERY numbered requirement in the description
- For integration milestones: test the actual integration path, not just that components build
- If a requirement needs Docker/infra: verify script must at minimum check the infrastructure
  exists (compose file present, binary builds and runs, config files valid)
- NEVER use generic `cargo build && cargo test && cargo fmt --check` for a milestone whose
  description requires specific integration behavior — that's a verify-description gap
- Each numbered requirement should map to a section in the verify script with a comment:
  ```bash
  # Requirement 1: binary accepts connections
  cargo build --bin my-service
  timeout 5 ./target/debug/my-service --check || exit 1
  # Requirement 2: SELECT 1 works end-to-end
  ...
  ```

## Phase 3.5: Validate milestone traceability

Before writing verify scripts, validate that review features form a complete gate.

For each review feature:
1. List its numbered requirements (from description)
2. Map each requirement → implementation feature(s) that deliver it
3. Confirm ALL mapped features are in `depends_on`
4. Flag any requirement with no delivering feature — create the feature or drop the requirement

Present the traceability matrix to the user before proceeding:

```markdown
### r001: M1 Milestone
| # | Requirement | Delivered by | Verified by |
|---|-------------|-------------|-------------|
| 1 | Binary accepts connections | f003 | r001.sh: connection test |
| 2 | SELECT 1 end-to-end | f004 | r001.sh: query test |
| 3 | Docker compose includes service | f002 | r001.sh: compose check |
| 4 | Code quality clean | f001-f004 | r001.sh: fmt + clippy |
```

If any cell is empty, the plan is incomplete. Fix before proceeding.

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
5. **Milestone traceability check**: For each review feature, confirm the traceability matrix
   from Phase 3.5 is complete — every requirement has a delivering feature, every delivering
   feature is in `depends_on`, and the verify script tests each requirement

## Priority ordering

Lower number = higher priority. Features with unmet deps are auto-skipped.
Group by scope, order by data flow within scope.
POC features should have the lowest priority numbers (run first).
