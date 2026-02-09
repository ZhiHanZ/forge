use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ForgeConfig {
    pub project: ProjectConfig,
    #[serde(default)]
    pub forge: ForgeSettings,
    #[serde(default)]
    pub principles: Principles,
    #[serde(default)]
    pub scopes: BTreeMap<String, Scope>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectConfig {
    pub name: String,
    #[serde(default)]
    pub stack: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ForgeSettings {
    #[serde(default = "default_max_agents")]
    pub max_agents: usize,
    #[serde(default = "default_budget")]
    pub budget_per_session: f64,
    #[serde(default)]
    pub roles: RoleConfig,
}

impl Default for ForgeSettings {
    fn default() -> Self {
        Self {
            max_agents: default_max_agents(),
            budget_per_session: default_budget(),
            roles: RoleConfig::default(),
        }
    }
}

/// Each role independently picks its backend and model.
/// Mix Claude and Codex freely across roles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleConfig {
    /// Executor: implements features. Needs strong coding ability.
    #[serde(default = "default_role_protocol")]
    pub protocol: RoleSpec,
    /// Reviewer: post-session feedback formatting + triage. Cheap & fast.
    #[serde(default = "default_role_orchestrating")]
    pub orchestrating: RoleSpec,
    /// Architect: design doc analysis + feature decomposition.
    #[serde(default = "default_role_planning")]
    pub planning: RoleSpec,
    /// Replanning: context-aware plan modification.
    #[serde(default = "default_role_adjusting")]
    pub adjusting: RoleSpec,
}

impl Default for RoleConfig {
    fn default() -> Self {
        Self {
            protocol: default_role_protocol(),
            orchestrating: default_role_orchestrating(),
            planning: default_role_planning(),
            adjusting: default_role_adjusting(),
        }
    }
}

/// A backend + model pair. Backend-specific model names
/// (e.g. "sonnet" for Claude, "o3" for Codex).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleSpec {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default = "default_model_sonnet")]
    pub model: String,
}

fn default_max_agents() -> usize {
    4
}
fn default_budget() -> f64 {
    5.0
}
fn default_backend() -> String {
    "claude".into()
}
fn default_model_sonnet() -> String {
    "sonnet".into()
}
fn default_role_protocol() -> RoleSpec {
    RoleSpec { backend: "claude".into(), model: "sonnet".into() }
}
fn default_role_orchestrating() -> RoleSpec {
    RoleSpec { backend: "claude".into(), model: "haiku".into() }
}
fn default_role_planning() -> RoleSpec {
    RoleSpec { backend: "claude".into(), model: "sonnet".into() }
}
fn default_role_adjusting() -> RoleSpec {
    RoleSpec { backend: "claude".into(), model: "sonnet".into() }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Principles {
    #[serde(default)]
    pub readability: String,
    #[serde(default)]
    pub proof: String,
    #[serde(default)]
    pub style: String,
    #[serde(default)]
    pub boundaries: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scope {
    pub owns: Vec<String>,
    #[serde(default)]
    pub api: String,
    #[serde(default)]
    pub upstream: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read forge.toml: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse forge.toml: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("failed to serialize forge.toml: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl ForgeConfig {
    pub fn load(project_dir: &Path) -> Result<Self, ConfigError> {
        let path = project_dir.join("forge.toml");
        let content = std::fs::read_to_string(&path)?;
        let config: ForgeConfig = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn save(&self, project_dir: &Path) -> Result<(), ConfigError> {
        let path = project_dir.join("forge.toml");
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Generate minimal forge.toml for a new project.
    pub fn scaffold(name: &str, stack: &str) -> Self {
        Self {
            project: ProjectConfig {
                name: name.into(),
                stack: stack.into(),
            },
            forge: ForgeSettings::default(),
            principles: Principles {
                readability: "Code understood in one read after an all nighter".into(),
                proof: "Tests prove code works, not test that it works".into(),
                style: "Follow a style even in private projects".into(),
                boundaries: "Divide at abstraction boundaries. APIs guide communication.".into(),
            },
            scopes: BTreeMap::new(),
        }
    }

    /// List scope names sorted.
    pub fn scope_names(&self) -> Vec<&str> {
        self.scopes.keys().map(|s| s.as_str()).collect()
    }

    /// Get files owned by a scope.
    pub fn scope_owns(&self, scope: &str) -> Option<&[String]> {
        self.scopes.get(scope).map(|s| s.owns.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TOML: &str = r#"
[project]
name = "my-app"
stack = "Rust, axum, sqlx"

[forge]
max_agents = 4
budget_per_session = 5.0

[forge.roles.protocol]
backend = "claude"
model = "sonnet"

[forge.roles.orchestrating]
backend = "claude"
model = "haiku"

[forge.roles.planning]
backend = "codex"
model = "o3"

[forge.roles.adjusting]
backend = "claude"
model = "sonnet"

[principles]
readability = "Code understood in one read after an all nighter"
proof = "Tests prove code works, not test that it works"
style = "Follow a style even in private projects"
boundaries = "Divide at abstraction boundaries. APIs guide communication."

[scopes.data-model]
owns = ["src/models/"]
api = "src/models/mod.rs"

[scopes.auth]
owns = ["src/auth/"]
api = "src/auth/mod.rs"
upstream = ["data-model"]
"#;

    #[test]
    fn parse_full_config() {
        let config: ForgeConfig = toml::from_str(SAMPLE_TOML).unwrap();
        assert_eq!(config.project.name, "my-app");
        assert_eq!(config.project.stack, "Rust, axum, sqlx");
        assert_eq!(config.forge.max_agents, 4);
        assert_eq!(config.forge.budget_per_session, 5.0);
        // Each role picks its own backend + model
        assert_eq!(config.forge.roles.protocol.backend, "claude");
        assert_eq!(config.forge.roles.protocol.model, "sonnet");
        assert_eq!(config.forge.roles.orchestrating.backend, "claude");
        assert_eq!(config.forge.roles.orchestrating.model, "haiku");
        assert_eq!(config.forge.roles.planning.backend, "codex");
        assert_eq!(config.forge.roles.planning.model, "o3");
        assert!(!config.principles.readability.is_empty());
        assert_eq!(config.scopes.len(), 2);
    }

    #[test]
    fn parse_minimal_config() {
        let toml_str = r#"
[project]
name = "bare"
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.project.name, "bare");
        assert_eq!(config.forge.max_agents, 4);
        assert_eq!(config.forge.budget_per_session, 5.0);
        // Defaults: all roles use claude
        assert_eq!(config.forge.roles.protocol.backend, "claude");
        assert_eq!(config.forge.roles.protocol.model, "sonnet");
        assert_eq!(config.forge.roles.orchestrating.model, "haiku");
        assert!(config.scopes.is_empty());
    }

    #[test]
    fn scope_names_sorted() {
        let config: ForgeConfig = toml::from_str(SAMPLE_TOML).unwrap();
        let names = config.scope_names();
        assert_eq!(names, vec!["auth", "data-model"]);
    }

    #[test]
    fn scope_upstream_deps() {
        let config: ForgeConfig = toml::from_str(SAMPLE_TOML).unwrap();
        let auth = &config.scopes["auth"];
        assert_eq!(auth.upstream, vec!["data-model"]);
        let dm = &config.scopes["data-model"];
        assert!(dm.upstream.is_empty());
    }

    #[test]
    fn scaffold_creates_default() {
        let config = ForgeConfig::scaffold("test-app", "Rust");
        assert_eq!(config.project.name, "test-app");
        assert_eq!(config.project.stack, "Rust");
        assert_eq!(config.forge.max_agents, 4);
        assert!(!config.principles.readability.is_empty());
        assert!(config.scopes.is_empty());
        // All roles default to claude
        assert_eq!(config.forge.roles.protocol.backend, "claude");
        assert_eq!(config.forge.roles.orchestrating.backend, "claude");
    }

    #[test]
    fn mix_backends_across_roles() {
        let toml_str = r#"
[project]
name = "mixed"

[forge.roles.protocol]
backend = "codex"
model = "o3"

[forge.roles.orchestrating]
backend = "claude"
model = "haiku"
"#;
        let config: ForgeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.forge.roles.protocol.backend, "codex");
        assert_eq!(config.forge.roles.protocol.model, "o3");
        assert_eq!(config.forge.roles.orchestrating.backend, "claude");
        assert_eq!(config.forge.roles.orchestrating.model, "haiku");
        // Unspecified roles get defaults
        assert_eq!(config.forge.roles.planning.backend, "claude");
        assert_eq!(config.forge.roles.planning.model, "sonnet");
    }

    #[test]
    fn roundtrip_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let config = ForgeConfig::scaffold("roundtrip", "Rust, axum");
        config.save(dir.path()).unwrap();
        let loaded = ForgeConfig::load(dir.path()).unwrap();
        assert_eq!(config, loaded);
    }

    #[test]
    fn load_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let result = ForgeConfig::load(dir.path());
        assert!(result.is_err());
    }
}
