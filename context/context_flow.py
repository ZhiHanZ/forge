"""CocoIndex pipeline for building per-feature context packages.

This flow extracts file-level metadata (function signatures, types, imports)
from source files, then assembles per-feature context packages that agents
read instead of re-scanning the codebase each session.

Usage:
    cocoindex update context/context_flow.py

Environment variables:
    FORGE_PROJECT_DIR  — project root (default: cwd)
    COCOINDEX_DB       — database URL for CocoIndex state
"""

import json
import os
from pathlib import Path

import cocoindex

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
    """Load execution memory for a feature (previous attempt history)."""
    mem_path = EXEC_MEMORY_DIR / f"{feature_id}.json"
    data = _load_json(mem_path)
    if not data or not isinstance(data, dict):
        return ""
    attempts = data.get("attempts", [])
    if not attempts:
        return ""
    lines = ["## Previous Attempts"]
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
    return "\n".join(lines)


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
# CocoIndex components
# ---------------------------------------------------------------------------

@cocoindex.op.function(memo=True)
def extract_file_info(file_content: str, file_path: str) -> FileMapInfo:
    """Extract function signatures, types, imports, and summary from a source file.

    This uses LLM-based extraction with memoization — unchanged files
    are not re-processed.
    """
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


def compile_package(
    feature: dict,
    file_infos: dict[str, FileMapInfo],
    config: dict | None,
) -> str:
    """Assemble a context package markdown for a single feature.

    This is a mechanical assembly step (no LLM) — it combines:
    - Feature metadata
    - Scope file maps (signatures, types)
    - Relevant knowledge entries
    - Execution memory (retry history)
    """
    lines = [f"# Context Package: {feature['id']}"]
    lines.append(f"\n**Description**: {feature.get('description', '')}")
    lines.append(f"**Scope**: {feature.get('scope', 'unknown')}")

    # Scope files
    scope_files = _get_scope_files(feature, config)
    if scope_files:
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


@cocoindex.op.function(memo=True)
def process_feature(feature_json: str, file_infos_json: str, config_json: str) -> str:
    """Process a single feature and produce its context package.

    Memoized: only re-runs when the feature, file infos, or config change.
    """
    feature = json.loads(feature_json)
    file_infos_raw = json.loads(file_infos_json)
    config = json.loads(config_json) if config_json else None

    file_infos = {k: FileMapInfo(**v) for k, v in file_infos_raw.items()}
    return compile_package(feature, file_infos, config)


# ---------------------------------------------------------------------------
# Main flow
# ---------------------------------------------------------------------------

def app_main():
    """Main entry point: extract file info, compile per-feature packages."""
    PACKAGES_DIR.mkdir(parents=True, exist_ok=True)
    EXEC_MEMORY_DIR.mkdir(parents=True, exist_ok=True)

    # Load features
    features_data = _load_json(PROJECT_DIR / "features.json")
    if not features_data:
        print("No features.json found or empty — skipping.")
        return

    features = features_data if isinstance(features_data, list) else features_data.get("features", [])

    # Filter to pending/claimed features only
    pending_features = [
        f for f in features
        if f.get("status") in ("pending", "claimed")
    ]
    if not pending_features:
        print("No pending features — nothing to package.")
        return

    # Load config
    config = _load_forge_config()

    # Collect and extract source file info
    source_files = _collect_source_files()
    file_infos: dict[str, FileMapInfo] = {}

    for rel_path in source_files:
        abs_path = PROJECT_DIR / rel_path
        try:
            content = abs_path.read_text()
        except Exception:
            continue
        info = extract_file_info(content, str(rel_path))
        file_infos[str(rel_path)] = info

    # Serialize for memoization
    file_infos_json = json.dumps({k: v.model_dump() for k, v in file_infos.items()})
    config_json = json.dumps(config) if config else ""

    # Compile packages
    for feature in pending_features:
        feature_json = json.dumps(feature)
        package_md = process_feature(feature_json, file_infos_json, config_json)

        out_path = PACKAGES_DIR / f"{feature['id']}.md"
        out_path.write_text(package_md)
        print(f"  Wrote {out_path.relative_to(PROJECT_DIR)}")


if __name__ == "__main__":
    app_main()
