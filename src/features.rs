use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeatureList {
    pub features: Vec<Feature>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Feature {
    pub id: String,
    #[serde(rename = "type")]
    pub feature_type: FeatureType,
    pub scope: String,
    pub description: String,
    pub verify: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default = "default_priority")]
    pub priority: u32,
    #[serde(default)]
    pub status: FeatureStatus,
    pub claimed_by: Option<String>,
    pub blocked_reason: Option<String>,
    /// Context entries relevant to this feature. Planner embeds these so agents
    /// don't need to scan INDEX.md — the right context is pushed, not pulled.
    /// Format: "category/slug" (e.g. "references/memory-management", "gotchas/sqlx-nullable")
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_hints: Vec<String>,
}

fn default_priority() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FeatureType {
    Implement,
    Review,
    Poc,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FeatureStatus {
    #[default]
    Pending,
    #[serde(alias = "in_progress")]
    Claimed,
    Done,
    Blocked,
}

#[derive(Debug, thiserror::Error)]
pub enum FeatureError {
    #[error("failed to read features.json: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse features.json: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("feature not found: {0}")]
    NotFound(String),
    #[error("feature {0} already claimed by {1}")]
    AlreadyClaimed(String, String),
    #[error("feature {0} has unmet dependencies: {1:?}")]
    DepsNotMet(String, Vec<String>),
}

impl FeatureList {
    pub fn load(project_dir: &Path) -> Result<Self, FeatureError> {
        let path = project_dir.join("features.json");
        let content = std::fs::read_to_string(&path)?;
        let list: FeatureList = serde_json::from_str(&content)?;
        Ok(list)
    }

    pub fn save(&self, project_dir: &Path) -> Result<(), FeatureError> {
        let path = project_dir.join("features.json");
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Find the highest-priority pending feature whose deps are all done.
    pub fn next_claimable(&self) -> Option<&Feature> {
        let done_ids: Vec<&str> = self
            .features
            .iter()
            .filter(|f| f.status == FeatureStatus::Done)
            .map(|f| f.id.as_str())
            .collect();

        self.features
            .iter()
            .filter(|f| f.status == FeatureStatus::Pending)
            .filter(|f| f.depends_on.iter().all(|dep| done_ids.contains(&dep.as_str())))
            .min_by_key(|f| f.priority)
    }

    /// Find up to N highest-priority pending features whose deps are all done.
    pub fn next_n_claimable(&self, n: usize) -> Vec<&Feature> {
        let done_ids: Vec<&str> = self
            .features
            .iter()
            .filter(|f| f.status == FeatureStatus::Done)
            .map(|f| f.id.as_str())
            .collect();

        let mut claimable: Vec<&Feature> = self
            .features
            .iter()
            .filter(|f| f.status == FeatureStatus::Pending)
            .filter(|f| f.depends_on.iter().all(|dep| done_ids.contains(&dep.as_str())))
            .collect();

        claimable.sort_by_key(|f| f.priority);
        claimable.truncate(n);
        claimable
    }

    /// Claim a feature for an agent. Returns error if already claimed or deps not met.
    pub fn claim(&mut self, feature_id: &str, agent_id: &str) -> Result<(), FeatureError> {
        let done_ids: Vec<String> = self
            .features
            .iter()
            .filter(|f| f.status == FeatureStatus::Done)
            .map(|f| f.id.clone())
            .collect();

        let feature = self
            .features
            .iter_mut()
            .find(|f| f.id == feature_id)
            .ok_or_else(|| FeatureError::NotFound(feature_id.into()))?;

        if let Some(claimed_by) = &feature.claimed_by {
            return Err(FeatureError::AlreadyClaimed(
                feature_id.into(),
                claimed_by.clone(),
            ));
        }

        let unmet: Vec<String> = feature
            .depends_on
            .iter()
            .filter(|dep| !done_ids.contains(dep))
            .cloned()
            .collect();

        if !unmet.is_empty() {
            return Err(FeatureError::DepsNotMet(feature_id.into(), unmet));
        }

        feature.status = FeatureStatus::Claimed;
        feature.claimed_by = Some(agent_id.into());
        Ok(())
    }

    /// Mark a feature as done.
    pub fn mark_done(&mut self, feature_id: &str) -> Result<(), FeatureError> {
        let feature = self
            .features
            .iter_mut()
            .find(|f| f.id == feature_id)
            .ok_or_else(|| FeatureError::NotFound(feature_id.into()))?;
        feature.status = FeatureStatus::Done;
        Ok(())
    }

    /// Mark a feature as blocked with a reason.
    pub fn mark_blocked(
        &mut self,
        feature_id: &str,
        reason: &str,
    ) -> Result<(), FeatureError> {
        let feature = self
            .features
            .iter_mut()
            .find(|f| f.id == feature_id)
            .ok_or_else(|| FeatureError::NotFound(feature_id.into()))?;
        feature.status = FeatureStatus::Blocked;
        feature.blocked_reason = Some(reason.into());
        Ok(())
    }

    /// Reopen a feature (verify failed after agent said done).
    pub fn reopen(&mut self, feature_id: &str) -> Result<(), FeatureError> {
        let feature = self
            .features
            .iter_mut()
            .find(|f| f.id == feature_id)
            .ok_or_else(|| FeatureError::NotFound(feature_id.into()))?;
        feature.status = FeatureStatus::Pending;
        feature.claimed_by = None;
        feature.blocked_reason = None;
        Ok(())
    }

    /// Summary counts by status.
    pub fn status_counts(&self) -> StatusCounts {
        let mut counts = StatusCounts::default();
        for f in &self.features {
            match f.status {
                FeatureStatus::Pending => counts.pending += 1,
                FeatureStatus::Claimed => counts.claimed += 1,
                FeatureStatus::Done => counts.done += 1,
                FeatureStatus::Blocked => counts.blocked += 1,
            }
        }
        counts.total = self.features.len();
        counts
    }

    /// Find the next feature to work on after completing `completed_id`.
    /// Prefers features whose depends_on includes `completed_id` (just-unblocked),
    /// then falls back to global priority order.
    pub fn next_after(&self, completed_id: &str) -> Option<&Feature> {
        let done_ids: Vec<&str> = self
            .features
            .iter()
            .filter(|f| f.status == FeatureStatus::Done)
            .map(|f| f.id.as_str())
            .collect();

        let claimable = |f: &&Feature| -> bool {
            f.status == FeatureStatus::Pending
                && f.depends_on.iter().all(|dep| done_ids.contains(&dep.as_str()))
        };

        // First: features that directly depend on the completed feature
        let unblocked = self
            .features
            .iter()
            .filter(claimable)
            .filter(|f| f.depends_on.iter().any(|dep| dep == completed_id))
            .min_by_key(|f| f.priority);

        if unblocked.is_some() {
            return unblocked;
        }

        // Fallback: global priority order
        self.features
            .iter()
            .filter(claimable)
            .min_by_key(|f| f.priority)
    }

    /// Return IDs of all currently claimable features (pending with all deps done).
    pub fn claimable_ids(&self) -> Vec<&str> {
        let done_ids: Vec<&str> = self
            .features
            .iter()
            .filter(|f| f.status == FeatureStatus::Done)
            .map(|f| f.id.as_str())
            .collect();

        self.features
            .iter()
            .filter(|f| f.status == FeatureStatus::Pending)
            .filter(|f| f.depends_on.iter().all(|dep| done_ids.contains(&dep.as_str())))
            .map(|f| f.id.as_str())
            .collect()
    }

    /// Return claimable features grouped by the earliest milestone they unblock.
    /// Milestones are review features, sorted by priority (nearest first).
    /// Each entry is (milestone_id, vec_of_claimable_ids_sorted_by_priority).
    /// Features not in any milestone's dep tree are returned under milestone_id = "".
    pub fn milestone_claimable(&self) -> Vec<(&str, Vec<&str>)> {
        use std::collections::{HashMap, HashSet, VecDeque};

        let claimable_set: HashSet<&str> = self.claimable_ids().into_iter().collect();
        if claimable_set.is_empty() {
            return vec![];
        }

        let feature_map: HashMap<&str, &Feature> =
            self.features.iter().map(|f| (f.id.as_str(), f)).collect();

        // Milestones: incomplete review features sorted by priority
        let mut milestones: Vec<&Feature> = self
            .features
            .iter()
            .filter(|f| f.feature_type == FeatureType::Review && f.status != FeatureStatus::Done)
            .collect();
        milestones.sort_by_key(|f| f.priority);

        let mut result = Vec::new();
        let mut assigned: HashSet<&str> = HashSet::new();

        for ms in &milestones {
            // BFS over transitive deps to find claimable frontier
            let mut frontier = Vec::new();
            let mut visited = HashSet::new();
            let mut queue = VecDeque::new();
            for dep in &ms.depends_on {
                queue.push_back(dep.as_str());
            }
            while let Some(dep_id) = queue.pop_front() {
                if !visited.insert(dep_id) {
                    continue;
                }
                if claimable_set.contains(dep_id) && !assigned.contains(dep_id) {
                    frontier.push(dep_id);
                    assigned.insert(dep_id);
                }
                if let Some(feat) = feature_map.get(dep_id) {
                    for sub_dep in &feat.depends_on {
                        queue.push_back(sub_dep.as_str());
                    }
                }
            }
            frontier
                .sort_by_key(|id| feature_map.get(id).map(|f| f.priority).unwrap_or(u32::MAX));
            if !frontier.is_empty() {
                result.push((ms.id.as_str(), frontier));
            }
        }

        // Orphans: claimable but not in any milestone's dep tree
        let mut orphans: Vec<&str> = claimable_set
            .iter()
            .filter(|id| !assigned.contains(**id))
            .copied()
            .collect();
        orphans.sort_by_key(|id| feature_map.get(id).map(|f| f.priority).unwrap_or(u32::MAX));
        if !orphans.is_empty() {
            result.push(("", orphans));
        }

        result
    }

    /// Check if all features are done.
    pub fn all_done(&self) -> bool {
        self.features.iter().all(|f| f.status == FeatureStatus::Done)
    }
}

#[derive(Debug, Default)]
pub struct StatusCounts {
    pub total: usize,
    pub pending: usize,
    pub claimed: usize,
    pub done: usize,
    pub blocked: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_features() -> FeatureList {
        FeatureList {
            features: vec![
                Feature {
                    id: "f001".into(),
                    feature_type: FeatureType::Implement,
                    scope: "data-model".into(),
                    description: "Create User struct".into(),
                    verify: "./scripts/verify/f001.sh".into(),
                    depends_on: vec![],
                    priority: 1,
                    status: FeatureStatus::Pending,
                    claimed_by: None,
                    blocked_reason: None,
                    context_hints: vec![],
                },
                Feature {
                    id: "f002".into(),
                    feature_type: FeatureType::Implement,
                    scope: "auth".into(),
                    description: "Add login endpoint".into(),
                    verify: "./scripts/verify/f002.sh".into(),
                    depends_on: vec!["f001".into()],
                    priority: 2,
                    status: FeatureStatus::Pending,
                    claimed_by: None,
                    blocked_reason: None,
                    context_hints: vec![],
                },
                Feature {
                    id: "f003".into(),
                    feature_type: FeatureType::Review,
                    scope: "data-model".into(),
                    description: "Review data-model boundaries".into(),
                    verify: "./scripts/verify/review-dm.sh".into(),
                    depends_on: vec!["f001".into()],
                    priority: 3,
                    status: FeatureStatus::Pending,
                    claimed_by: None,
                    blocked_reason: None,
                    context_hints: vec![],
                },
            ],
        }
    }

    #[test]
    fn next_claimable_respects_deps() {
        let list = sample_features();
        let next = list.next_claimable().unwrap();
        // f001 has no deps, should be claimable. f002/f003 depend on f001.
        assert_eq!(next.id, "f001");
    }

    #[test]
    fn next_claimable_after_dep_done() {
        let mut list = sample_features();
        list.claim("f001", "agent-1").unwrap();
        list.mark_done("f001").unwrap();

        let next = list.next_claimable().unwrap();
        // f002 (priority 2) should be claimable now
        assert_eq!(next.id, "f002");
    }

    #[test]
    fn claim_prevents_double_claim() {
        let mut list = sample_features();
        list.claim("f001", "agent-1").unwrap();
        let result = list.claim("f001", "agent-2");
        assert!(matches!(result, Err(FeatureError::AlreadyClaimed(_, _))));
    }

    #[test]
    fn claim_checks_deps() {
        let mut list = sample_features();
        let result = list.claim("f002", "agent-1");
        assert!(matches!(result, Err(FeatureError::DepsNotMet(_, _))));
    }

    #[test]
    fn mark_blocked_sets_reason() {
        let mut list = sample_features();
        list.claim("f001", "agent-1").unwrap();
        list.mark_blocked("f001", "stuck on compile error").unwrap();

        let f = list.features.iter().find(|f| f.id == "f001").unwrap();
        assert_eq!(f.status, FeatureStatus::Blocked);
        assert_eq!(f.blocked_reason.as_deref(), Some("stuck on compile error"));
    }

    #[test]
    fn reopen_resets_status() {
        let mut list = sample_features();
        list.claim("f001", "agent-1").unwrap();
        list.mark_done("f001").unwrap();
        list.reopen("f001").unwrap();

        let f = list.features.iter().find(|f| f.id == "f001").unwrap();
        assert_eq!(f.status, FeatureStatus::Pending);
        assert!(f.claimed_by.is_none());
    }

    #[test]
    fn status_counts() {
        let mut list = sample_features();
        list.claim("f001", "agent-1").unwrap();
        list.mark_done("f001").unwrap();

        let counts = list.status_counts();
        assert_eq!(counts.total, 3);
        assert_eq!(counts.done, 1);
        assert_eq!(counts.pending, 2);
    }

    #[test]
    fn all_done_false_when_pending() {
        let list = sample_features();
        assert!(!list.all_done());
    }

    #[test]
    fn all_done_true_when_complete() {
        let mut list = sample_features();
        for id in ["f001", "f002", "f003"] {
            list.claim(id, "agent-1").ok();
            list.mark_done(id).unwrap();
        }
        assert!(list.all_done());
    }

    #[test]
    fn roundtrip_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let list = sample_features();
        list.save(dir.path()).unwrap();
        let loaded = FeatureList::load(dir.path()).unwrap();
        assert_eq!(list, loaded);
    }

    #[test]
    fn json_serialization_format() {
        let list = sample_features();
        let json = serde_json::to_string_pretty(&list).unwrap();
        assert!(json.contains("\"type\": \"implement\""));
        assert!(json.contains("\"status\": \"pending\""));
        assert!(json.contains("\"depends_on\""));
        // context_hints should be omitted when empty
        assert!(!json.contains("context_hints"));
    }

    #[test]
    fn context_hints_roundtrip() {
        let mut list = sample_features();
        list.features[0].context_hints = vec![
            "references/memory-management".into(),
            "gotchas/sqlx-nullable".into(),
        ];
        let dir = tempfile::tempdir().unwrap();
        list.save(dir.path()).unwrap();
        let loaded = FeatureList::load(dir.path()).unwrap();
        assert_eq!(loaded.features[0].context_hints, vec![
            "references/memory-management",
            "gotchas/sqlx-nullable",
        ]);
        // Features without hints should deserialize with empty vec
        assert!(loaded.features[1].context_hints.is_empty());
    }

    #[test]
    fn next_n_claimable_returns_up_to_n() {
        let mut list = sample_features();
        // Make f001 done so f002 and f003 become claimable
        list.claim("f001", "agent-1").unwrap();
        list.mark_done("f001").unwrap();

        let claimable = list.next_n_claimable(5);
        // f002 (priority 2) and f003 (priority 3) should be claimable
        assert_eq!(claimable.len(), 2);
        assert_eq!(claimable[0].id, "f002");
        assert_eq!(claimable[1].id, "f003");
    }

    #[test]
    fn next_n_claimable_respects_limit() {
        let mut list = sample_features();
        list.claim("f001", "agent-1").unwrap();
        list.mark_done("f001").unwrap();

        let claimable = list.next_n_claimable(1);
        assert_eq!(claimable.len(), 1);
        assert_eq!(claimable[0].id, "f002"); // highest priority
    }

    #[test]
    fn poc_feature_serializes() {
        let poc = Feature {
            id: "p001".into(),
            feature_type: FeatureType::Poc,
            scope: "data-model".into(),
            description: "Validate thrift parsing approach".into(),
            verify: "./scripts/verify/p001.sh".into(),
            depends_on: vec![],
            priority: 1,
            status: FeatureStatus::Pending,
            claimed_by: None,
            blocked_reason: None,
            context_hints: vec!["references/rpc-patterns".into()],
        };
        let json = serde_json::to_string_pretty(&poc).unwrap();
        assert!(json.contains("\"type\": \"poc\""));
        let roundtrip: Feature = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.feature_type, FeatureType::Poc);
        assert_eq!(roundtrip.context_hints, vec!["references/rpc-patterns"]);
    }

    #[test]
    fn next_after_prefers_unblocked() {
        let mut list = sample_features();
        // Add an unrelated low-priority feature with no deps
        list.features.push(Feature {
            id: "f099".into(),
            feature_type: FeatureType::Implement,
            scope: "misc".into(),
            description: "Unrelated low-pri feature".into(),
            verify: "./scripts/verify/f099.sh".into(),
            depends_on: vec![],
            priority: 1, // Lower number = higher priority than f002(2)/f003(3)
            status: FeatureStatus::Pending,
            claimed_by: None,
            blocked_reason: None,
            context_hints: vec![],
        });
        // Complete f001
        list.claim("f001", "agent-1").unwrap();
        list.mark_done("f001").unwrap();

        // next_after("f001") should prefer f002/f003 (depend on f001) over f099
        let next = list.next_after("f001").unwrap();
        assert!(
            next.id == "f002" || next.id == "f003",
            "expected f002 or f003 (depends on f001), got {}",
            next.id,
        );
        // Specifically f002 because it has lower priority number (2 < 3)
        assert_eq!(next.id, "f002");
    }

    #[test]
    fn next_after_falls_back() {
        let mut list = sample_features();
        // Add an unrelated feature with no deps
        list.features.push(Feature {
            id: "f099".into(),
            feature_type: FeatureType::Implement,
            scope: "misc".into(),
            description: "Unrelated feature".into(),
            verify: "./scripts/verify/f099.sh".into(),
            depends_on: vec![],
            priority: 1,
            status: FeatureStatus::Pending,
            claimed_by: None,
            blocked_reason: None,
            context_hints: vec![],
        });
        // Complete f001, then claim f002 and f003 (the direct dependents)
        list.claim("f001", "agent-1").unwrap();
        list.mark_done("f001").unwrap();
        list.claim("f002", "agent-2").unwrap();
        list.mark_done("f002").unwrap();
        list.claim("f003", "agent-3").unwrap();
        list.mark_done("f003").unwrap();

        // next_after("f003") — no features depend on f003, falls back to global
        let next = list.next_after("f003").unwrap();
        assert_eq!(next.id, "f099");
    }

    #[test]
    fn next_after_handles_unknown_id() {
        let mut list = sample_features();
        list.claim("f001", "agent-1").unwrap();
        list.mark_done("f001").unwrap();

        // "f999" doesn't exist — should fall back to global priority
        let next = list.next_after("f999").unwrap();
        assert_eq!(next.id, "f002");
    }

    #[test]
    fn not_found_error() {
        let mut list = sample_features();
        let result = list.claim("f999", "agent-1");
        assert!(matches!(result, Err(FeatureError::NotFound(_))));
    }

    #[test]
    fn claimable_ids_no_deps_done() {
        let list = sample_features();
        // Only f001 is claimable (no deps)
        let ids = list.claimable_ids();
        assert_eq!(ids, vec!["f001"]);
    }

    #[test]
    fn claimable_ids_after_done() {
        let mut list = sample_features();
        list.claim("f001", "agent-1").unwrap();
        list.mark_done("f001").unwrap();
        // f002 and f003 both depend on f001, now claimable
        let mut ids = list.claimable_ids();
        ids.sort();
        assert_eq!(ids, vec!["f002", "f003"]);
    }

    #[test]
    fn claimable_ids_empty_when_all_done() {
        let mut list = sample_features();
        for id in ["f001", "f002", "f003"] {
            list.claim(id, "agent-1").ok();
            list.mark_done(id).unwrap();
        }
        assert!(list.claimable_ids().is_empty());
    }

    #[test]
    fn claimable_ids_excludes_claimed() {
        let mut list = sample_features();
        list.claim("f001", "agent-1").unwrap();
        // f001 is now claimed, not pending — should not appear
        assert!(list.claimable_ids().is_empty());
    }

    #[test]
    fn milestone_claimable_groups_by_review() {
        let mut list = FeatureList {
            features: vec![
                Feature {
                    id: "f030".into(),
                    feature_type: FeatureType::Implement,
                    scope: "sql".into(),
                    description: "Done dep".into(),
                    verify: "true".into(),
                    depends_on: vec![],
                    priority: 50,
                    status: FeatureStatus::Done,
                    claimed_by: None,
                    blocked_reason: None,
                    context_hints: vec![],
                },
                Feature {
                    id: "f042".into(),
                    feature_type: FeatureType::Implement,
                    scope: "sql".into(),
                    description: "UNION".into(),
                    verify: "true".into(),
                    depends_on: vec!["f030".into()],
                    priority: 139,
                    status: FeatureStatus::Pending,
                    claimed_by: None,
                    blocked_reason: None,
                    context_hints: vec![],
                },
                Feature {
                    id: "f065".into(),
                    feature_type: FeatureType::Implement,
                    scope: "node".into(),
                    description: "Role mgmt".into(),
                    verify: "true".into(),
                    depends_on: vec![],
                    priority: 100,
                    status: FeatureStatus::Pending,
                    claimed_by: None,
                    blocked_reason: None,
                    context_hints: vec![],
                },
                Feature {
                    id: "r104".into(),
                    feature_type: FeatureType::Review,
                    scope: "all".into(),
                    description: "M4 review".into(),
                    verify: "true".into(),
                    depends_on: vec!["f042".into()],
                    priority: 154,
                    status: FeatureStatus::Pending,
                    claimed_by: None,
                    blocked_reason: None,
                    context_hints: vec![],
                },
                Feature {
                    id: "r105".into(),
                    feature_type: FeatureType::Review,
                    scope: "all".into(),
                    description: "M5 review".into(),
                    verify: "true".into(),
                    depends_on: vec!["f065".into()],
                    priority: 179,
                    status: FeatureStatus::Pending,
                    claimed_by: None,
                    blocked_reason: None,
                    context_hints: vec![],
                },
            ],
        };

        let groups = list.milestone_claimable();
        // r104 (priority 154) is the nearest milestone
        assert_eq!(groups[0].0, "r104");
        assert_eq!(groups[0].1, vec!["f042"]);
        // r105 (priority 179) is next
        assert_eq!(groups[1].0, "r105");
        assert_eq!(groups[1].1, vec!["f065"]);
    }

    #[test]
    fn milestone_claimable_transitive() {
        let list = FeatureList {
            features: vec![
                Feature {
                    id: "f030".into(),
                    feature_type: FeatureType::Implement,
                    scope: "sql".into(),
                    description: "Done".into(),
                    verify: "true".into(),
                    depends_on: vec![],
                    priority: 50,
                    status: FeatureStatus::Done,
                    claimed_by: None,
                    blocked_reason: None,
                    context_hints: vec![],
                },
                Feature {
                    id: "f043".into(),
                    feature_type: FeatureType::Implement,
                    scope: "sql".into(),
                    description: "INSERT".into(),
                    verify: "true".into(),
                    depends_on: vec!["f030".into()],
                    priority: 140,
                    status: FeatureStatus::Pending,
                    claimed_by: None,
                    blocked_reason: None,
                    context_hints: vec![],
                },
                Feature {
                    id: "f044".into(),
                    feature_type: FeatureType::Implement,
                    scope: "http".into(),
                    description: "Stream load".into(),
                    verify: "true".into(),
                    depends_on: vec!["f043".into()],
                    priority: 141,
                    status: FeatureStatus::Pending,
                    claimed_by: None,
                    blocked_reason: None,
                    context_hints: vec![],
                },
                Feature {
                    id: "r104".into(),
                    feature_type: FeatureType::Review,
                    scope: "all".into(),
                    description: "M4 review".into(),
                    verify: "true".into(),
                    // r104 depends on f044, which transitively depends on f043
                    depends_on: vec!["f044".into()],
                    priority: 154,
                    status: FeatureStatus::Pending,
                    claimed_by: None,
                    blocked_reason: None,
                    context_hints: vec![],
                },
            ],
        };

        let groups = list.milestone_claimable();
        // f043 is claimable and transitively blocks r104 (via f044)
        assert_eq!(groups[0].0, "r104");
        assert_eq!(groups[0].1, vec!["f043"]);
    }
}
