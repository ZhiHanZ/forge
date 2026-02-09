---
name: forge-protocol
description: >
  Autonomous coding agent protocol for forge-managed development loops.
  Follow this protocol when working in a forge project: claim features from
  features.json, implement them, run verify scripts, write context entries,
  and commit. Used automatically during forge run sessions.
---

# Forge Protocol

You are an autonomous coding agent in a forge-managed development loop.
Follow this protocol exactly. One feature per session. No scope creep.

## Session start

1. Read `features.json` — find your assigned feature (or claim the highest-priority
   unblocked pending feature)
2. Read `context/` — check decisions, gotchas, patterns, references
3. Read `feedback/` — check current test state
4. Read your feature's scope in `forge.toml` — understand what you own

## Implementation loop

1. Implement the feature within its scope's owned files
2. Run the feature's `verify` command: check the `verify` field in features.json
3. If verify passes: mark status `"done"`, go to Session end
4. If verify fails: read the error output, fix, retry
5. After 10+ failed attempts: mark status `"blocked"`, add `blocked_reason`, exit

## Session end

1. Write discoveries to context:
   - `context/decisions/{slug}.md` — why you made a choice
   - `context/gotchas/{slug}.md` — pitfalls you encountered
   - `context/patterns/{slug}.md` — conventions worth following
2. Save external knowledge (see [CONTEXT-WRITING.md](CONTEXT-WRITING.md))
3. Commit all changes and push

## Hard rules

- **One feature per session.** Never scope-creep.
- **Never modify features you didn't claim.** Other agents own those.
- **Never weaken verify commands.** They're the trust layer.
- **Never modify files outside your scope** unless the API surface requires it.
- **Stuck 10+ attempts → blocked.** Add reason, exit. Don't spin.

## Claiming (parallel agents)

When multiple agents run in parallel, features are claimed atomically via git.
See [CLAIMING.md](CLAIMING.md) for the protocol.
