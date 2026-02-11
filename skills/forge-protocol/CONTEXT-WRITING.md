# Context Writing Guide

Write context entries that help future agents avoid re-work. Each entry is one
markdown file in `context/{category}/`.

## Categories

| Category | What goes here | Example |
|----------|---------------|---------|
| `decisions/` | Why a choice was made | "Use Vec<u8> not ring buffer: simpler, in-memory only" |
| `gotchas/` | Pitfalls you encountered | "sqlx requires Option<T> for nullable columns" |
| `patterns/` | Code conventions worth following | "Handler signature: async fn(State, Json<Req>) -> Result<Json<Res>>" |
| `poc/` | POC outcomes (goal, result, learnings, design impact) | "Thrift parsing: nom works, 2x faster than pest" |
| `references/` | External knowledge for rediscovery | Distilled blog posts, library patterns, paper insights |

## Writing good entries

- One concept per file. Name the file after the concept: `use-vec-not-ringbuffer.md`
- Be concrete: include code snippets, not descriptions
- Include WHY, not just WHAT — future agents need the reasoning
- Keep under 50 lines for decisions/gotchas/patterns/poc, under 300 for references

## POC outcome protocol

When completing a POC feature, write `context/poc/{feature-id}.md` with this structure:

```markdown
# POC: {description}

**Goal**: What we're trying to validate (one sentence)
**Result**: pass | fail | partial
**Learnings**: What we discovered (concrete findings, not vague impressions)
**Design Impact**: Which DESIGN.md sections need updating and how
```

Keep under 50 lines. Focus on actionable findings. If the POC failed, explain what
would need to change for it to work (different tech, different approach, reduced scope).

## Reference protocol

After using WebSearch or WebFetch for something useful:

1. Write `context/references/{topic}.md`
2. Include YAML frontmatter:
   ```
   ---
   source: https://example.com/article
   tags: [topic, subtopic]
   ---
   ```
3. Include: key points, code patterns, how it applies to this project
4. Keep under 300 lines — working knowledge, not the full source
5. Future agents read this (~200 tokens) instead of re-fetching (~2000 tokens)

After exploring unfamiliar library code:

1. Write `context/references/{library}-patterns.md`
2. Include concrete code snippets showing API usage
3. Link to source file/line for deep dives

## Execution Memory

At the end of each session, write `feedback/exec-memory/{your_feature_id}.json` to record
what you attempted. This is consumed by context packages to help future agents avoid
repeating failed approaches.

**Schema:**

```json
{
  "feature_id": "f001",
  "attempts": [
    {
      "number": 1,
      "summary": "Brief description of what was attempted",
      "failed_reason": "Why it failed (empty string if succeeded)",
      "discoveries": ["List of things learned during this attempt"]
    }
  ]
}
```

**Rules:**
- Always write this file, even on success (with empty `failed_reason`)
- Append to the `attempts` array if the file already exists
- Keep summaries concise (one sentence each)
- `discoveries` should be actionable facts, not vague observations
