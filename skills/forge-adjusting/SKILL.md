---
name: forge-adjusting
description: >
  Replan a forge project mid-execution. Use when the user wants to change the
  feature plan, add new features, modify scope, or adjust priorities after
  development has started. Handles POC failures and architecture pivots.
  Triggers: "forge adjust", "replan", "add features", "change plan".
---

# Forge Adjusting

You are helping the user modify a forge project's plan mid-execution.
Read existing state before proposing changes. Never break completed work.

## Phase 1: Orientation

Read these files:
- `features.json` — current feature list with statuses
- `forge.toml` — scopes and principles
- `context/` — decisions, gotchas, patterns, poc, references from completed work
- `feedback/` — current test state, session reviews

Report to the user:
- Feature counts: done / in-progress / pending / blocked
- POC outcomes: which passed, which failed, which are pending
- Blocked features and their reasons
- Accumulated context summary (count per category)

## Phase 2: POC failure handling

When `context/poc/{id}.md` has `Result: fail`:

1. Read the outcome — understand what failed and why
2. Identify features that depend on the failed POC (via `depends_on`)
3. Propose alternatives to the user:
   - New POC with different technology/approach
   - Reduce scope to avoid the problematic area
   - Adjust architecture to work around the limitation
4. Update DESIGN.md unknowns:
   - Mark failed POC as `[!]` with explanation
   - Add new `[ ]` if proposing a new POC approach

## Phase 3: Discuss changes with user

The user will describe what they want to change. Common scenarios:
- Add new features ("add pagination to search")
- Modify existing pending features
- Change priorities
- Add or modify scopes
- Respond to blocked features
- React to POC outcomes (pivot, proceed, or abandon)

## Phase 4: Apply changes

Rules for modifying features.json:
- **Never change `done` features** — they're verified and committed
- **Never remove `done` features** — they may be dependencies
- **Blocked features**: can be unblocked, modified, or replaced
- **Pending features**: can be modified, reprioritized, or removed
- **Claimed features**: warn the user — an agent may be working on it
- **New features**: add with proper deps, verify commands, scope

Principle enforcement for new features:
- Every verify script includes `cargo fmt --check` and `cargo clippy -- -D warnings` (P3)
- Implementation verify scripts include specific tests that prove the deliverable (P2)
- POC verify scripts check for `context/poc/{id}.md` (P2)
- New scopes: add to forge.toml first, then reference in features

### Milestone integrity rules

When modifying or creating review features (milestones):

- **Never invent statuses**: No "CONDITIONAL PASS", "PARTIAL", or other custom states.
  The system has 4 states: pending, claimed, done, blocked. Use them. If a milestone has
  conditions, those conditions are features in `depends_on` — not prose in a review document.
- **Conditions must be features**: If a milestone review or adjustment identifies conditions
  for completion, each condition MUST become a feature with its own verify script, and the
  milestone's `depends_on` MUST include it. Prose conditions are unenforceable.
- **Unmet conditions = not done**: If a milestone's conditions are unmet, set it to `pending`
  (if condition features exist in `depends_on` and the system will gate it) or `blocked`
  (if condition features don't exist yet and need to be created). Never `done`.
- **Verify must match description**: Review feature verify scripts must test the actual
  integration gate described, not just `cargo build/test/fmt/clippy`. If the description
  says "end-to-end query works", the verify script must test that path.
- **Traceability**: After modifications, present a traceability matrix for each affected
  milestone showing: requirement → delivering feature → verify coverage. Empty cells = gap.

## Phase 5: Validate

- All `depends_on` references point to valid feature IDs
- All `scope` values exist in forge.toml
- All `verify` scripts exist and are executable
- No circular dependencies
- DESIGN.md unknowns updated if POC pivot occurred
- Review the changes with the user before committing

**Definition of Done**: Updated features.json, verify scripts for new features,
DESIGN.md unknowns updated if POC pivot.
