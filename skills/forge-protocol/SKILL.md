---
name: forge-protocol
description: >
  Autonomous coding agent protocol for forge-managed development loops.
  Follow this protocol when working in a forge project: claim features from
  features.json, implement them, run verify scripts, write context entries,
  and commit. Used automatically during forge run sessions.
---

# Forge Protocol

You are an expert engineer working in a continuous development loop. You are one of
many agents working on this project in shifts. You have NO memory of previous sessions —
rely entirely on the file system and git history.

## Phase 1: Orientation (MANDATORY before any code)

1. **Check location**: `pwd`, confirm working directory
2. **Review history**: Read `feedback/session-review.md` (last session's review). Run `git log --oneline -n 5`
3. **Review requirements**: Read `features.json` — find your assigned feature or highest-priority unblocked pending
4. **Read context package**: If `context/packages/{your_feature_id}.md` exists, read it first — it contains pre-compiled scope files, interfaces, relevant context, and previous attempt history. If it doesn't exist, read your feature's `context_hints` array entries (`context/{hint}.md`).
5. **Read feedback pointers**: If `feedback/session-review.md` has `SEE:` lines, read those too — the reviewer pointed you to context that would have helped the previous agent.
6. **Fallback browsing**: If hints don't cover your needs, see [CONTEXT-READING.md](CONTEXT-READING.md) for how to scan INDEX.md and explore further.
7. **Establish baseline**: Run the feature's verify script. If it already passes, pick next feature. If the main branch is broken, fix that FIRST.

## Phase 2: Execution (one feature only)

1. Claim the feature (see [CLAIMING.md](CLAIMING.md) for multi-agent protocol)
2. Read your feature's scope in `forge.toml` — understand what you own
3. **Scope**: Work on ONLY this single feature. Do not refactor unrelated code.
4. Implement within scope's owned files (check `forge.toml` scopes)
5. Follow the 4 principles from CLAUDE.md — especially P3 (style) and P4 (boundaries)

## Phase 3: Testing

Write tests that prove your feature works. See [TESTING.md](TESTING.md) for the 7 rules
of meaningful tests.

Key requirements:
- Tests assert behavior, not implementation details
- Names describe business logic (`parse_config_rejects_missing_field`, not `test_parse`)
- Cover edge cases: empty inputs, boundary values, error states
- Tests must survive refactoring of internals

## Phase 4: Verification

1. Run the feature's verify command end-to-end
2. If PASS → mark status `"done"` in features.json
3. If FAIL → read error, fix, retry
4. After 10+ attempts → mark `"blocked"` with reason, exit
5. Only mark done when verification succeeds. Never remove tests or weaken verify.

## Phase 5: POC features

If your feature has `"type": "poc"`:

Same flow as above, but additionally write `context/poc/{feature-id}.md`:

```markdown
# POC: {description}

**Goal**: What we're trying to validate
**Result**: pass | fail | partial
**Learnings**: What we discovered
**Design Impact**: How this affects the design (which DESIGN.md sections to update)
```

The verify script checks this file exists. A POC is done when:
- The viability test runs (pass or fail — both are valid outcomes)
- The outcome file exists with all 4 sections filled

## Phase 6: Handoff (clean state for next agent)

1. Write discoveries to `context/{decisions,gotchas,patterns}/` (see [CONTEXT-WRITING.md](CONTEXT-WRITING.md))
2. External knowledge → `context/references/`
3. Write execution memory with **tactics** to `feedback/exec-memory/{feature_id}.json`:
   - `attempts` — what you tried, what failed, what you discovered
   - `tactics` — which context you used, your approach, test strategy, insights, perf notes
   - See [CONTEXT-WRITING.md](CONTEXT-WRITING.md#execution-memory) for full schema
4. Commit all with descriptive message
5. Push. Exit.

Your context entries, tactics, and execution memory become part of the completed
feature's package — downstream features (via `depends_on`) receive your API surface,
decisions, approach, and test strategy automatically.

## Hard rules

- **One feature per session.** Never scope-creep.
- **Never modify features you didn't claim.** Other agents own those.
- **Never weaken verify commands.** They're the trust layer.
- **Never modify files outside your scope** unless the API surface requires it.
- **POC features MUST write `context/poc/{id}.md`.** This is a deliverable, not optional.
- **Stuck 10+ attempts → blocked.** Add reason, exit. Don't spin.

**Definition of Done**: Feature verify passes, status is "done" (or "blocked" with reason),
all changes committed and pushed, context entries written.
