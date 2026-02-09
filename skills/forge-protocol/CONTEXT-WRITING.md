# Context Writing Guide

Write context entries that help future agents avoid re-work. Each entry is one
markdown file in `context/{category}/`.

## Categories

| Category | What goes here | Example |
|----------|---------------|---------|
| `decisions/` | Why a choice was made | "Use Vec<u8> not ring buffer: simpler, in-memory only" |
| `gotchas/` | Pitfalls you encountered | "sqlx requires Option<T> for nullable columns" |
| `patterns/` | Code conventions worth following | "Handler signature: async fn(State, Json<Req>) -> Result<Json<Res>>" |
| `references/` | External knowledge for rediscovery | Distilled blog posts, library patterns, paper insights |

## Writing good entries

- One concept per file. Name the file after the concept: `use-vec-not-ringbuffer.md`
- Be concrete: include code snippets, not descriptions
- Include WHY, not just WHAT — future agents need the reasoning
- Keep under 50 lines for decisions/gotchas/patterns, under 300 for references

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
