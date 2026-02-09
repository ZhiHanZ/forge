use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The five context categories.
const CATEGORIES: &[&str] = &["decisions", "gotchas", "patterns", "poc", "references"];

#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("failed to access context directory: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown context category: {0}")]
    UnknownCategory(String),
}

/// A single context entry (one markdown file).
#[derive(Debug, Clone)]
pub struct ContextEntry {
    pub category: String,
    pub slug: String,
    pub path: PathBuf,
}

/// Manages the context/ directory.
pub struct ContextManager {
    root: PathBuf,
}

impl ContextManager {
    pub fn new(project_dir: &Path) -> Self {
        Self {
            root: project_dir.join("context"),
        }
    }

    /// Create context/ with all subdirectories.
    pub fn init(&self) -> Result<(), ContextError> {
        for cat in CATEGORIES {
            std::fs::create_dir_all(self.root.join(cat))?;
        }
        Ok(())
    }

    /// List all entries across all categories.
    pub fn list_all(&self) -> Result<Vec<ContextEntry>, ContextError> {
        let mut entries = Vec::new();
        for cat in CATEGORIES {
            entries.extend(self.list_category(cat)?);
        }
        Ok(entries)
    }

    /// List entries in a single category.
    pub fn list_category(&self, category: &str) -> Result<Vec<ContextEntry>, ContextError> {
        validate_category(category)?;
        let dir = self.root.join(category);
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                let slug = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                entries.push(ContextEntry {
                    category: category.into(),
                    slug,
                    path,
                });
            }
        }
        entries.sort_by(|a, b| a.slug.cmp(&b.slug));
        Ok(entries)
    }

    /// Read the content of a context entry.
    pub fn read_entry(&self, category: &str, slug: &str) -> Result<String, ContextError> {
        validate_category(category)?;
        let path = self.root.join(category).join(format!("{slug}.md"));
        let content = std::fs::read_to_string(&path)?;
        Ok(content)
    }

    /// Write a context entry. Overwrites if exists.
    pub fn write_entry(
        &self,
        category: &str,
        slug: &str,
        content: &str,
    ) -> Result<PathBuf, ContextError> {
        validate_category(category)?;
        let dir = self.root.join(category);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{slug}.md"));
        std::fs::write(&path, content)?;
        Ok(path)
    }

    /// Count entries per category.
    pub fn counts(&self) -> Result<BTreeMap<String, usize>, ContextError> {
        let mut map = BTreeMap::new();
        for cat in CATEGORIES {
            let count = self.list_category(cat)?.len();
            map.insert((*cat).to_string(), count);
        }
        Ok(map)
    }

    /// Generate the context section for CLAUDE.md.
    /// Lists entry slugs per category, one line each.
    pub fn generate_claude_section(&self) -> Result<String, ContextError> {
        let all = self.list_all()?;
        if all.is_empty() {
            return Ok(String::new());
        }

        let mut section = String::from("## Context\n");

        let mut by_category: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for entry in &all {
            by_category
                .entry(&entry.category)
                .or_default()
                .push(&entry.slug);
        }

        for (cat, slugs) in &by_category {
            let label = capitalize(cat);
            section.push_str(&format!("{label}: {}\n", slugs.join(", ")));
        }
        section.push_str("Read context/{category}/{name}.md for details.\n");
        Ok(section)
    }

    /// Generate context/INDEX.md — one-line-per-entry table of contents.
    /// Agents scan this (~1 token/entry) to decide what to read in full.
    pub fn generate_index(&self) -> Result<String, ContextError> {
        let mut index = String::from("# Context Index\n\n");
        let mut total = 0usize;

        for cat in CATEGORIES {
            let entries = self.list_category(cat)?;
            if entries.is_empty() {
                continue;
            }
            let label = capitalize(cat);
            index.push_str(&format!("## {label} ({} entries)\n", entries.len()));
            for entry in &entries {
                let summary = self.first_heading(&entry.path);
                index.push_str(&format!("- {}: {}\n", entry.slug, summary));
            }
            index.push('\n');
            total += entries.len();
        }

        if total == 0 {
            return Ok(String::new());
        }

        Ok(index)
    }

    /// Write INDEX.md to context/.
    pub fn write_index(&self) -> Result<(), ContextError> {
        let index = self.generate_index()?;
        if index.is_empty() {
            return Ok(());
        }
        std::fs::write(self.root.join("INDEX.md"), &index)?;
        Ok(())
    }

    /// Extract first heading or first non-empty line from a file.
    fn first_heading(&self, path: &Path) -> String {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return "(unreadable)".into(),
        };
        // Skip YAML frontmatter
        let body = if content.starts_with("---") {
            content
                .splitn(3, "---")
                .nth(2)
                .unwrap_or(&content)
        } else {
            &content
        };
        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Strip markdown heading prefix
            let stripped = trimmed.trim_start_matches('#').trim();
            if !stripped.is_empty() {
                // Truncate to 80 chars
                return if stripped.len() > 80 {
                    format!("{}...", &stripped[..77])
                } else {
                    stripped.to_string()
                };
            }
        }
        "(empty)".into()
    }

    /// Write a reference entry with YAML frontmatter.
    pub fn write_reference(
        &self,
        slug: &str,
        source_url: &str,
        tags: &[&str],
        body: &str,
    ) -> Result<PathBuf, ContextError> {
        let tags_str = tags
            .iter()
            .map(|t| format!("{t}"))
            .collect::<Vec<_>>()
            .join(", ");
        let content = format!(
            "---\nsource: {source_url}\ntags: [{tags_str}]\n---\n\n{body}"
        );
        self.write_entry("references", slug, &content)
    }
}

fn validate_category(category: &str) -> Result<(), ContextError> {
    if CATEGORIES.contains(&category) {
        Ok(())
    } else {
        Err(ContextError::UnknownCategory(category.into()))
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, ContextManager) {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ContextManager::new(dir.path());
        mgr.init().unwrap();
        (dir, mgr)
    }

    #[test]
    fn init_creates_directories() {
        let (dir, _mgr) = setup();
        for cat in CATEGORIES {
            assert!(dir.path().join("context").join(cat).is_dir());
        }
    }

    #[test]
    fn write_and_read_entry() {
        let (_dir, mgr) = setup();
        mgr.write_entry("decisions", "use-vec", "Use Vec<u8> for buffer")
            .unwrap();
        let content = mgr.read_entry("decisions", "use-vec").unwrap();
        assert_eq!(content, "Use Vec<u8> for buffer");
    }

    #[test]
    fn list_category_returns_sorted() {
        let (_dir, mgr) = setup();
        mgr.write_entry("gotchas", "z-last", "last").unwrap();
        mgr.write_entry("gotchas", "a-first", "first").unwrap();

        let entries = mgr.list_category("gotchas").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].slug, "a-first");
        assert_eq!(entries[1].slug, "z-last");
    }

    #[test]
    fn list_all_across_categories() {
        let (_dir, mgr) = setup();
        mgr.write_entry("decisions", "d1", "decision 1").unwrap();
        mgr.write_entry("gotchas", "g1", "gotcha 1").unwrap();
        mgr.write_entry("references", "r1", "ref 1").unwrap();

        let all = mgr.list_all().unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn counts_per_category() {
        let (_dir, mgr) = setup();
        mgr.write_entry("decisions", "d1", "x").unwrap();
        mgr.write_entry("decisions", "d2", "y").unwrap();
        mgr.write_entry("gotchas", "g1", "z").unwrap();

        let counts = mgr.counts().unwrap();
        assert_eq!(counts["decisions"], 2);
        assert_eq!(counts["gotchas"], 1);
        assert_eq!(counts["patterns"], 0);
        assert_eq!(counts["poc"], 0);
        assert_eq!(counts["references"], 0);
    }

    #[test]
    fn unknown_category_rejected() {
        let (_dir, mgr) = setup();
        let result = mgr.write_entry("unknown", "test", "content");
        assert!(matches!(result, Err(ContextError::UnknownCategory(_))));
    }

    #[test]
    fn generate_claude_section_empty() {
        let (_dir, mgr) = setup();
        let section = mgr.generate_claude_section().unwrap();
        assert!(section.is_empty());
    }

    #[test]
    fn generate_claude_section_with_entries() {
        let (_dir, mgr) = setup();
        mgr.write_entry("decisions", "use-vec", "x").unwrap();
        mgr.write_entry("gotchas", "head-tail", "y").unwrap();
        mgr.write_entry("references", "bf-tree-blog", "z").unwrap();

        let section = mgr.generate_claude_section().unwrap();
        assert!(section.contains("## Context"));
        assert!(section.contains("Decisions: use-vec"));
        assert!(section.contains("Gotchas: head-tail"));
        assert!(section.contains("References: bf-tree-blog"));
        assert!(section.contains("Read context/{category}/{name}.md for details."));
    }

    #[test]
    fn write_reference_with_frontmatter() {
        let (_dir, mgr) = setup();
        mgr.write_reference(
            "bf-tree-blog",
            "https://example.com/bf-tree",
            &["bf-tree", "rust"],
            "## Key Points\n- Circular buffer uses Vec<u8>",
        )
        .unwrap();

        let content = mgr.read_entry("references", "bf-tree-blog").unwrap();
        assert!(content.contains("source: https://example.com/bf-tree"));
        assert!(content.contains("tags: [bf-tree, rust]"));
        assert!(content.contains("## Key Points"));
    }

    #[test]
    fn overwrite_existing_entry() {
        let (_dir, mgr) = setup();
        mgr.write_entry("decisions", "d1", "version 1").unwrap();
        mgr.write_entry("decisions", "d1", "version 2").unwrap();
        let content = mgr.read_entry("decisions", "d1").unwrap();
        assert_eq!(content, "version 2");
    }

    #[test]
    fn write_and_read_poc_entry() {
        let (_dir, mgr) = setup();
        mgr.write_entry("poc", "p001-thrift-parsing", "# POC: Thrift Parsing\n\n**Goal**: Validate thrift parser crate.\n**Result**: pass\n**Learnings**: nom-based parser works.\n**Design Impact**: Use nom for all parsing.")
            .unwrap();
        let content = mgr.read_entry("poc", "p001-thrift-parsing").unwrap();
        assert!(content.contains("**Result**: pass"));
    }

    #[test]
    fn generate_index_empty_returns_empty() {
        let (_dir, mgr) = setup();
        let index = mgr.generate_index().unwrap();
        assert!(index.is_empty());
    }

    #[test]
    fn generate_index_lists_entries_with_headings() {
        let (_dir, mgr) = setup();
        mgr.write_entry("decisions", "use-vec", "# Use Vec<u8> for buffer\nSimpler than ring buffer.")
            .unwrap();
        mgr.write_entry("gotchas", "sqlx-nullable", "# sqlx requires Option<T> for nullable\nOtherwise panics.")
            .unwrap();

        let index = mgr.generate_index().unwrap();
        assert!(index.contains("# Context Index"));
        assert!(index.contains("## Decisions (1 entries)"));
        assert!(index.contains("- use-vec: Use Vec<u8> for buffer"));
        assert!(index.contains("## Gotchas (1 entries)"));
        assert!(index.contains("- sqlx-nullable: sqlx requires Option<T> for nullable"));
    }

    #[test]
    fn generate_index_skips_frontmatter() {
        let (_dir, mgr) = setup();
        mgr.write_reference("bf-tree", "https://example.com", &["rust"], "Key points about bf-tree")
            .unwrap();

        let index = mgr.generate_index().unwrap();
        assert!(index.contains("- bf-tree: Key points about bf-tree"));
        // Should NOT contain frontmatter
        assert!(!index.contains("source:"));
    }

    #[test]
    fn write_index_creates_file() {
        let (_dir, mgr) = setup();
        mgr.write_entry("decisions", "d1", "# Decision one").unwrap();
        mgr.write_index().unwrap();

        let path = _dir.path().join("context/INDEX.md");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("- d1: Decision one"));
    }

    #[test]
    fn ignores_non_md_files() {
        let (_dir, mgr) = setup();
        mgr.write_entry("decisions", "d1", "x").unwrap();
        // Write a non-md file directly
        let txt_path = _dir.path().join("context/decisions/notes.txt");
        std::fs::write(&txt_path, "not markdown").unwrap();

        let entries = mgr.list_category("decisions").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].slug, "d1");
    }
}
