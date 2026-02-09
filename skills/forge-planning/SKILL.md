---
name: forge-planning
description: >
  Analyze project design docs and generate features.json for forge orchestration.
  Use when setting up a new forge project, when the user wants to plan features,
  or when they say "forge planning", "plan features", or "generate features".
  Checks design doc coverage, helps fill gaps, decomposes into features with
  verify scripts and dependency ordering.
---

# Forge Planning

You are helping the user plan a forge project. This is an interactive conversation —
discuss architecture decisions, help fill design doc gaps, then generate features.json.

## Step 1: Read project state

Read these files (skip missing ones):
- `forge.toml` — project config, scopes, principles
- `DESIGN.md` — project design document
- `features.json` — existing features (if replanning)
- `context/` — any existing decisions, gotchas, patterns, references

## Step 2: Check design doc coverage

Read [COVERAGE.md](COVERAGE.md) for the 7 sections checklist. Report to the user:
- Which sections are present and well-defined
- Which sections are missing or vague
- Why each missing section matters for agent execution

**Do not proceed to feature generation until the user is satisfied with coverage.**
Help them fill gaps — suggest concrete types, error strategies, constraints.

## Step 3: Generate features

Once the design doc is sufficient:

1. Decompose into features — one per concrete deliverable
2. Each feature needs:
   ```json
   {
     "id": "f001",
     "type": "implement",
     "scope": "scope-name",
     "description": "Concrete deliverable",
     "verify": "./scripts/verify/f001.sh",
     "depends_on": [],
     "priority": 1,
     "status": "pending",
     "claimed_by": null,
     "blocked_reason": null
   }
   ```
3. Set `depends_on` based on data flow (what must exist before this works)
4. Add `review` type features between implementation batches
5. Write verify scripts that test the feature's actual deliverable

## Step 4: Write verify scripts

For each feature, write `scripts/verify/{id}.sh`:
- Must return exit code 0 on success, non-zero on failure
- Test the actual deliverable, not just "does it compile"
- Include: build check, specific tests, boundary checks where relevant
- Review features: check fmt, clippy, scope boundary imports

## Step 5: Validate

Run `forge verify` to confirm all scripts are executable and valid.
Review the full features.json with the user before finishing.

## Feature types

- `implement` — write code, verify with tests
- `review` — check boundaries, update docs, verify with lint + import checks

## Priority ordering

Lower number = higher priority. Features with unmet deps are auto-skipped.
Group by scope, order by data flow within scope.
