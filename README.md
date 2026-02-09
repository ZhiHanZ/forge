# Forge

Rust CLI that orchestrates autonomous coding agents in a managed development loop. Spawns agents, assigns features, verifies results, accumulates context, repeats. Single binary, zero external dependencies.

## How It Works

Forge runs a loop:

```
┌──────────────────────────────────────────┐
│  1. Pick next unblocked feature          │
│  2. Spawn agent (Claude / Codex)         │
│  3. Agent implements + commits           │
│  4. CLI runs verify script (exit code)   │
│  5. Pass → done. Fail → reopen.          │
│  6. Orchestrating agent reviews session  │
│  7. Git sync, repeat                     │
└──────────────────────────────────────────┘
```

Key ideas:

- **Verify scripts are the trust layer** — agents claim "done", scripts confirm with exit codes
- **Context accumulates in git** — decisions, gotchas, patterns, and references persist across sessions
- **Skills are markdown** — four skills (planning, protocol, orchestrating, adjusting) installed as `.claude/skills/`
- **CLI never calls an LLM** — all intelligence is in the skills, CLI is pure orchestration
- **Multi-agent via git worktrees** — parallel agents work in isolated branches, merged back after each round

## Install

```bash
cargo install --path .
```

Requires [Claude Code](https://docs.anthropic.com/en/docs/claude-code) or [Codex CLI](https://github.com/openai/codex) installed and authenticated.

## Quick Start

```bash
# 1. Initialize a forge project
forge init "My REST API"

# 2. Plan features (interactive, inside Claude Code)
#    Run /forge-planning in a Claude Code session

# 3. Run the autonomous loop
forge run
```

After `forge init`, your project has:

```
forge.toml              # project config
features.json           # task list (fill via /forge-planning)
CLAUDE.md               # agent instructions (~40 lines)
AGENTS.md               # same, for non-Claude agents
context/                # decisions/, gotchas/, patterns/, references/
feedback/               # verify reports, session reviews
scripts/verify/         # one script per feature (exit 0 = pass)
.claude/skills/         # 4 skills installed
```

## Commands

```bash
forge init <description>    # scaffold project
forge run                   # start development loop (1 agent)
forge run --agents 3        # parallel agents with git worktrees
forge run --max-sessions 10 # cap iterations
forge verify                # run all verify scripts
forge status                # show feature progress + context counts
forge stop                  # graceful stop after current session
forge logs agent-1          # tail agent log
forge logs agent-1 -t 100   # last 100 lines
```

## Configuration

`forge.toml`:

```toml
[project]
name = "my-app"
stack = "Rust, axum, sqlx"

# Each role picks its own backend + model
[forge.roles.protocol]       # executor: implements features
backend = "claude"
model = "sonnet"

[forge.roles.orchestrating]  # reviewer: post-session feedback
backend = "claude"
model = "haiku"

[forge.roles.planning]       # architect: feature decomposition
backend = "codex"
model = "o3"

[principles]
readability = "Code understood in one read after an all nighter"
proof = "Tests prove code works, not test that it works"
style = "Follow a style even in private projects"
boundaries = "Divide at abstraction boundaries. APIs guide communication."

[scopes.data-model]
owns = ["src/models/", "src/schema/"]

[scopes.auth]
owns = ["src/auth/"]
upstream = ["data-model"]
```

Supported backends: `claude` (Claude Code), `codex` (OpenAI Codex CLI), or any binary name for custom backends.

## Features File

`features.json` — the task list agents work from:

```json
{
  "features": [
    {
      "id": "f001",
      "type": "implement",
      "scope": "data-model",
      "description": "Create User struct with validation",
      "verify": "./scripts/verify/f001.sh",
      "depends_on": [],
      "priority": 1,
      "status": "pending",
      "claimed_by": null,
      "blocked_reason": null
    }
  ]
}
```

Statuses: `pending` → `claimed` → `done` (or `blocked`).

## The Loop in Detail

**Single agent** (`forge run`):
1. Load `features.json`, find highest-priority unblocked pending feature
2. Spawn agent subprocess (`claude --print` or `codex exec`)
3. Agent reads CLAUDE.md, claims feature, implements, runs verify, commits
4. CLI runs all verify scripts, writes `feedback/last-verify.json`
5. Failed features get reopened automatically
6. Git pull to sync
7. Orchestrating agent reviews the session, writes `feedback/session-review.md` and context entries
8. Next iteration

**Multi-agent** (`forge run --agents N`):
1. Pick up to N claimable features
2. Create git worktrees (one per agent, isolated branches)
3. Spawn N agents in parallel
4. Wait for all to finish
5. Merge branches back into main (conflicts → abort + retry next round)
6. Verify, orchestrate, repeat

## Skills

Four markdown skills installed in `.claude/skills/`:

| Skill | Mode | Purpose |
|-------|------|---------|
| `forge-planning` | Interactive | Design doc → feature decomposition |
| `forge-protocol` | Automated | Agent executor: claim → implement → verify → commit |
| `forge-orchestrating` | Automated | Post-session review: feedback + context writing |
| `forge-adjusting` | Interactive | Replan based on new context |

## Development

```bash
cargo test        # 66 tests
cargo build       # debug build
cargo clippy      # lint
```
