---
name: forge-adjusting
description: >
  Replan a forge project mid-execution. Use when the user wants to change the
  feature plan, add new features, modify scope, or adjust priorities after
  development has started. Reads existing context and preserves completed work.
  Triggers: "forge adjust", "replan", "add features", "change plan".
---

# Forge Adjusting

You are helping the user modify a forge project's plan mid-execution.
Read existing state before proposing changes. Never break completed work.

## Step 1: Read current state

Read these files:
- `features.json` — current feature list with statuses
- `forge.toml` — scopes and principles
- `context/` — decisions, gotchas, patterns, references from completed work
- `feedback/` — current test state

Report to the user:
- How many features are done / in-progress / pending / blocked
- What context has been accumulated
- Any blocked features and their reasons

## Step 2: Discuss changes

The user will describe what they want to change. Common scenarios:
- Add new features ("add pagination to search")
- Modify existing pending features
- Change priorities
- Add or modify scopes
- Respond to blocked features

## Step 3: Apply changes

Rules for modifying features.json:
- **Never change `done` features** — they're verified and committed
- **Never remove `done` features** — they may be dependencies
- **Blocked features**: can be unblocked, modified, or replaced
- **Pending features**: can be modified, reprioritized, or removed
- **Claimed features**: warn the user — an agent may be working on it
- **New features**: add with proper deps, verify commands, scope
- **New scopes**: add to forge.toml first, then reference in features

## Step 4: Update verify scripts

For new or modified features, write verify scripts in `scripts/verify/`.
Ensure existing verify scripts for done features still pass: run `forge verify`.

## Step 5: Validate

- All `depends_on` references point to valid feature IDs
- All `scope` values exist in forge.toml
- All `verify` scripts exist and are executable
- No circular dependencies
- Review the changes with the user before committing
