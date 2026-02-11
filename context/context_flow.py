"""CocoIndex pipeline for building per-feature context packages.

This flow extracts file-level metadata (function signatures, types, imports)
from source files, then assembles per-feature context packages that agents
read instead of re-scanning the codebase each session.

Usage (with CocoIndex):
    cocoindex update .forge/context_flow.py

Usage (standalone, no memoization):
    python .forge/context_flow.py

Environment variables:
    FORGE_PROJECT_DIR        — project root (default: cwd)
    COCOINDEX_DATABASE_URL   — LMDB path for CocoIndex state
    FORGE_EXTRACT_MODEL      — LLM model for file extraction (default: gpt-4o-mini)
"""

import asyncio
import json
import os
import sys
from pathlib import Path

# CocoIndex is optional — works without it (no memoization).
try:
    import cocoindex
    HAS_COCOINDEX = True
except ImportError:
    HAS_COCOINDEX = False

sys.path.insert(0, str(Path(__file__).parent))
from context_models import FileMapInfo

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

PROJECT_DIR = Path(os.environ.get("FORGE_PROJECT_DIR", ".")).resolve()
CONTEXT_DIR = PROJECT_DIR / "context"
PACKAGES_DIR = CONTEXT_DIR / "packages"
FEEDBACK_DIR = PROJECT_DIR / "feedback"
EXEC_MEMORY_DIR = FEEDBACK_DIR / "exec-memory"

# Directories to skip when scanning source files
SKIP_DIRS = {
    ".git", ".forge", "node_modules", "target", "dist", "build",
    "__pycache__", ".venv", "venv", "references",
}

# File extensions to include
SOURCE_EXTS = {
    ".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".go", ".java",
    ".c", ".cpp", ".h", ".hpp", ".rb", ".swift", ".kt", ".scala",
}


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _load_json(path: Path) -> dict | list | None:
    """Load JSON file, returning None on any error."""
    try:
        return json.loads(path.read_text())
    except Exception:
        return None


def _collect_source_files() -> list[Path]:
    """Walk the project and return source file paths (relative to PROJECT_DIR)."""
    files = []
    for root, dirs, filenames in os.walk(PROJECT_DIR):
        # Prune skipped directories in-place
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
        for fname in filenames:
            p = Path(root) / fname
            if p.suffix in SOURCE_EXTS:
                files.append(p.relative_to(PROJECT_DIR))
    return sorted(files)


def _load_knowledge_entries() -> str:
    """Load all context knowledge entries as markdown sections."""
    sections = []
    categories = ["decisions", "gotchas", "patterns", "references", "poc"]
    for cat in categories:
        cat_dir = CONTEXT_DIR / cat
        if not cat_dir.is_dir():
            continue
        for md_file in sorted(cat_dir.glob("*.md")):
            if md_file.name == "INDEX.md":
                continue
            content = md_file.read_text().strip()
            if content:
                sections.append(f"### {cat}/{md_file.stem}\n{content}")
    return "\n\n".join(sections)


def _load_exec_memory(feature_id: str) -> str:
    """Load execution memory for a feature (attempts + tactics)."""
    mem_path = EXEC_MEMORY_DIR / f"{feature_id}.json"
    data = _load_json(mem_path)
    if not data or not isinstance(data, dict):
        return ""

    lines: list[str] = []

    # Attempts history
    attempts = data.get("attempts", [])
    if attempts:
        lines.append("## Previous Attempts")
        for a in attempts:
            num = a.get("number", "?")
            summary = a.get("summary", "")
            reason = a.get("failed_reason", "")
            lines.append(f"- Attempt {num}: {summary}")
            if reason:
                lines.append(f"  Failed: {reason}")
            discoveries = a.get("discoveries", [])
            for d in discoveries:
                lines.append(f"  - Discovered: {d}")

    # Session tactics — the reusable knowledge from implementation
    tactics = data.get("tactics")
    if tactics and isinstance(tactics, dict):
        lines.append("\n## Session Tactics")
        if tactics.get("approach"):
            lines.append(f"**Approach**: {tactics['approach']}")
        if tactics.get("test_strategy"):
            lines.append(f"**Test strategy**: {tactics['test_strategy']}")
        if tactics.get("verify_result"):
            lines.append(f"**Verify result**: {tactics['verify_result']}")
        if tactics.get("performance_notes"):
            lines.append(f"**Performance**: {tactics['performance_notes']}")
        context_used = tactics.get("context_used", [])
        if context_used:
            lines.append(f"**Context used**: {', '.join(context_used)}")
        key_files = tactics.get("key_files_read", [])
        if key_files:
            lines.append(f"**Key files**: {', '.join(key_files)}")
        insights = tactics.get("insights", [])
        if insights:
            lines.append("**Insights:**")
            for i in insights:
                lines.append(f"- {i}")

    return "\n".join(lines) if lines else ""


def _load_forge_config() -> dict | None:
    """Load forge.toml as dict (requires tomli or tomllib)."""
    toml_path = PROJECT_DIR / "forge.toml"
    if not toml_path.exists():
        return None
    try:
        import tomllib
    except ImportError:
        try:
            import tomli as tomllib  # type: ignore[no-redef]
        except ImportError:
            return None
    try:
        return tomllib.loads(toml_path.read_text())
    except Exception:
        return None


def _get_scope_files(feature: dict, config: dict | None) -> list[str]:
    """Get the list of owned files for a feature's scope from forge.toml."""
    scope_name = feature.get("scope", "")
    if not config or not scope_name:
        return []
    scopes = config.get("scopes", {})
    scope = scopes.get(scope_name, {})
    return scope.get("owns", [])


# ---------------------------------------------------------------------------
# LLM extraction (memoized when CocoIndex is available)
# ---------------------------------------------------------------------------

def _extract_file_info_llm(file_content: str, file_path: str) -> FileMapInfo:
    """Extract function signatures, types, imports, and summary from a source file."""
    import instructor
    import litellm

    client = instructor.from_litellm(litellm.completion)
    resp = client.chat.completions.create(
        model=os.environ.get("FORGE_EXTRACT_MODEL", "gpt-4o-mini"),
        response_model=FileMapInfo,
        messages=[
            {
                "role": "system",
                "content": (
                    "Extract public API information from this source file. "
                    "Include function signatures, public types, key imports, "
                    "and a one-line summary. Be precise and concise."
                ),
            },
            {
                "role": "user",
                "content": f"File: {file_path}\n\n```\n{file_content}\n```",
            },
        ],
        max_tokens=1000,
    )
    return resp


# Wrap with CocoIndex memoization if available
if HAS_COCOINDEX:
    extract_file_info = cocoindex.function(memo=True)(_extract_file_info_llm)
else:
    extract_file_info = _extract_file_info_llm


# ---------------------------------------------------------------------------
# Package compilation (pure Python, no LLM)
# ---------------------------------------------------------------------------

def _render_scope_files(
    feature: dict,
    file_infos: dict[str, FileMapInfo],
    config: dict | None,
) -> list[str]:
    """Render scope file maps as markdown lines."""
    lines: list[str] = []
    scope_files = _get_scope_files(feature, config)
    if not scope_files:
        return lines
    lines.append("\n## Scope Files")
    for sf in scope_files:
        info = file_infos.get(sf)
        if info:
            lines.append(f"\n### {info.name} ({info.lines} lines)")
            lines.append(f"{info.summary}")
            if info.public_functions:
                lines.append("\n**Functions:**")
                for fn in info.public_functions:
                    lines.append(f"- `{fn.signature}` — {fn.summary}")
            if info.public_types:
                lines.append("\n**Types:**")
                for t in info.public_types:
                    lines.append(f"- `{t.name}` — {t.summary}")
            if info.key_imports:
                lines.append(f"\n**Key imports:** {', '.join(info.key_imports)}")
        else:
            lines.append(f"\n### {sf}")
            lines.append("_(not yet analyzed)_")
    return lines


def _load_scope_context(scope: str) -> list[str]:
    """Load context entries (decisions, gotchas, patterns) related to a scope."""
    lines: list[str] = []
    categories = ["decisions", "gotchas", "patterns"]
    for cat in categories:
        cat_dir = CONTEXT_DIR / cat
        if not cat_dir.is_dir():
            continue
        for md_file in sorted(cat_dir.glob("*.md")):
            if md_file.name == "INDEX.md":
                continue
            if scope.lower().replace("-", "") in md_file.stem.lower().replace("-", ""):
                content = md_file.read_text().strip()
                if content:
                    lines.append(f"\n### {cat}/{md_file.stem}")
                    lines.append(content)
    return lines


def compile_completed_package(
    feature: dict,
    file_infos: dict[str, FileMapInfo],
    config: dict | None,
) -> str:
    """Assemble a completed-feature package — an API contract for dependents.

    When a feature is done, its package becomes a reusable knowledge artifact:
    - API surface: function signatures, types, imports from scope files
    - Context links: decisions, gotchas, patterns created during implementation
    - POC results: validation outcomes (if POC feature)
    - Discoveries: what the implementing agent learned

    This is useful for:
    1. Downstream features in this project (via depends_on)
    2. Other projects working on similar problems (portable knowledge)
    """
    lines = [f"# Completed: {feature['id']}"]
    lines.append(f"\n**Description**: {feature.get('description', '')}")
    lines.append(f"**Scope**: {feature.get('scope', 'unknown')}")
    lines.append("**Status**: done")

    lines.extend(_render_scope_files(feature, file_infos, config))

    scope = feature.get("scope", "")
    if scope:
        scope_context = _load_scope_context(scope)
        if scope_context:
            lines.append("\n## Decisions & Patterns")
            lines.extend(scope_context)

    hints = feature.get("context_hints", [])
    if hints:
        linked = []
        for hint in hints:
            hint_path = CONTEXT_DIR / f"{hint}.md"
            if hint_path.exists():
                content = hint_path.read_text().strip()
                if content:
                    linked.append(f"\n### {hint}")
                    linked.append(content)
        if linked:
            lines.append("\n## Linked Context")
            lines.extend(linked)

    poc_path = CONTEXT_DIR / "poc" / f"{feature['id']}.md"
    if poc_path.exists():
        content = poc_path.read_text().strip()
        if content:
            lines.append("\n## POC Results")
            lines.append(content)

    exec_mem = _load_exec_memory(feature["id"])
    if exec_mem:
        lines.append(f"\n{exec_mem}")

    return "\n".join(lines) + "\n"


def compile_package(
    feature: dict,
    file_infos: dict[str, FileMapInfo],
    config: dict | None,
    all_features: list[dict] | None = None,
) -> str:
    """Assemble a context package markdown for a single feature.

    Progressive disclosure for dependencies:
    - Tier 1: Summary table (~1 token per dep)
    - Tier 2: API surface — signatures + types only (~20 tokens per dep)
    - Tier 3: Pointer to full package (agent reads on demand)
    """
    lines = [f"# Context Package: {feature['id']}"]
    lines.append(f"\n**Description**: {feature.get('description', '')}")
    lines.append(f"**Scope**: {feature.get('scope', 'unknown')}")

    # Dependencies — progressive disclosure
    depends_on = feature.get("depends_on", [])
    if depends_on and all_features:
        features_by_id = {f["id"]: f for f in all_features}
        done_deps = [
            features_by_id[dep_id]
            for dep_id in depends_on
            if dep_id in features_by_id
            and features_by_id[dep_id].get("status") == "done"
        ]
        unmet = [
            dep_id for dep_id in depends_on
            if dep_id in features_by_id
            and features_by_id[dep_id].get("status") != "done"
        ]

        if done_deps or unmet:
            lines.append("\n## Dependencies")

            # Tier 1: Summary table
            lines.append("")
            lines.append("| Dep | Description | Scope | Status |")
            lines.append("|-----|-------------|-------|--------|")
            for dep in done_deps:
                lines.append(
                    f"| {dep['id']} | {dep.get('description', '')} "
                    f"| {dep.get('scope', '')} | done |"
                )
            for dep_id in unmet:
                dep = features_by_id.get(dep_id, {})
                lines.append(
                    f"| {dep_id} | {dep.get('description', '?')} "
                    f"| {dep.get('scope', '?')} | **pending** |"
                )

            # Tier 2: API surface per done dep
            for dep in done_deps:
                scope_files = _get_scope_files(dep, config)
                if not scope_files:
                    continue
                has_api = False
                for sf in scope_files:
                    info = file_infos.get(sf)
                    if info and (info.public_functions or info.public_types):
                        if not has_api:
                            lines.append(f"\n### {dep['id']} — API Surface")
                            has_api = True
                        if info.public_functions:
                            for fn in info.public_functions:
                                lines.append(f"- `{fn.signature}` — {fn.summary}")
                        if info.public_types:
                            for t in info.public_types:
                                lines.append(f"- `{t.name}` — {t.summary}")

            # Tier 3: Pointer to full packages
            full_paths = []
            for dep in done_deps:
                dep_pkg_path = PACKAGES_DIR / f"{dep['id']}.md"
                if dep_pkg_path.exists():
                    full_paths.append(f"`context/packages/{dep['id']}.md`")
            if full_paths:
                lines.append(
                    f"\n> **Deep dive**: For full tactics, decisions, and test strategy "
                    f"read {', '.join(full_paths)}"
                )

    # Scope files
    lines.extend(_render_scope_files(feature, file_infos, config))

    # Context hints
    hints = feature.get("context_hints", [])
    if hints:
        lines.append("\n## Relevant Context")
        for hint in hints:
            hint_path = CONTEXT_DIR / f"{hint}.md"
            if hint_path.exists():
                content = hint_path.read_text().strip()
                lines.append(f"\n### {hint}")
                lines.append(content)

    # Knowledge entries
    knowledge = _load_knowledge_entries()
    if knowledge:
        lines.append("\n## Project Knowledge")
        lines.append(knowledge)

    # Execution memory
    exec_mem = _load_exec_memory(feature["id"])
    if exec_mem:
        lines.append(f"\n{exec_mem}")

    return "\n".join(lines) + "\n"


def process_feature(
    feature: dict,
    file_infos: dict[str, FileMapInfo],
    config: dict | None,
    all_features: list[dict] | None = None,
    is_completed: bool = False,
) -> str:
    """Process a single feature and produce its context package."""
    if is_completed:
        return compile_completed_package(feature, file_infos, config)
    return compile_package(feature, file_infos, config, all_features)


# ---------------------------------------------------------------------------
# Main flow
# ---------------------------------------------------------------------------

def app_main():
    """Main entry point: extract file info, compile per-feature packages.

    Two-pass approach:
    1. Build completed packages for done features that are depended upon
    2. Build pending packages that include their completed dependency interfaces
    """
    PACKAGES_DIR.mkdir(parents=True, exist_ok=True)
    EXEC_MEMORY_DIR.mkdir(parents=True, exist_ok=True)

    # Load features
    features_data = _load_json(PROJECT_DIR / "features.json")
    if not features_data:
        print("No features.json found or empty — skipping.")
        return

    features = features_data if isinstance(features_data, list) else features_data.get("features", [])

    pending_features = [
        f for f in features
        if f.get("status") in ("pending", "claimed")
    ]

    # Find done features that are depended upon by any pending feature
    pending_deps: set[str] = set()
    for f in pending_features:
        pending_deps.update(f.get("depends_on", []))

    done_features = [
        f for f in features
        if f.get("status") == "done" and f.get("id") in pending_deps
    ]

    if not pending_features and not done_features:
        print("No features to package.")
        return

    # Load config
    config = _load_forge_config()

    # Collect and extract source file info (LLM-based, skipped if no API key)
    source_files = _collect_source_files()
    file_infos: dict[str, FileMapInfo] = {}

    has_llm = bool(os.environ.get("OPENAI_API_KEY") or os.environ.get("ANTHROPIC_API_KEY"))
    if has_llm:
        for rel_path in source_files:
            abs_path = PROJECT_DIR / rel_path
            try:
                content = abs_path.read_text()
            except Exception:
                continue
            try:
                info = extract_file_info(content, str(rel_path))
                file_infos[str(rel_path)] = info
            except Exception as e:
                print(f"  Skipping {rel_path}: {e}")
    else:
        print("  No LLM API key — skipping file extraction (packages will lack scope file maps)")

    # Pass 1: compile completed packages (API contracts for dependents)
    for feature in done_features:
        package_md = process_feature(
            feature, file_infos, config, features, is_completed=True,
        )
        out_path = PACKAGES_DIR / f"{feature['id']}.md"
        out_path.write_text(package_md)
        print(f"  Wrote {out_path.relative_to(PROJECT_DIR)} (completed)")

    # Pass 2: compile pending packages (includes dependency interfaces)
    for feature in pending_features:
        package_md = process_feature(
            feature, file_infos, config, features, is_completed=False,
        )
        out_path = PACKAGES_DIR / f"{feature['id']}.md"
        out_path.write_text(package_md)
        print(f"  Wrote {out_path.relative_to(PROJECT_DIR)}")


if __name__ == "__main__":
    app_main()
