# Feature Claiming Protocol

When multiple agents run in parallel, use atomic git-based claiming to avoid conflicts.

## How to claim

1. Read `features.json` — find highest-priority pending feature with all deps done
2. Set `claimed_by` to your agent ID, `status` to `"claimed"`
3. Commit: `git commit -am "claim f001"`
4. Push: `git push`
5. If push fails (another agent claimed simultaneously): `git pull --rebase`, pick next feature
6. If push succeeds: you own this feature, proceed with implementation

## Rules

- Only claim one feature at a time
- Never claim a feature with unmet `depends_on`
- Never unclaim another agent's feature
- If you finish and there are more features: exit session, let forge respawn you

## Agent ID

Your agent ID is set by forge in the environment variable `FORGE_AGENT_ID`.
Use this when setting `claimed_by` in features.json.
