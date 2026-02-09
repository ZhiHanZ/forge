use serde::Serialize;
use std::path::Path;
use std::process::Command;

use crate::features::{FeatureList, FeatureStatus};

#[derive(Debug)]
pub struct VerifyResult {
    pub feature_id: String,
    pub passed: bool,
    pub output: String,
}

/// JSON report written to feedback/last-verify.json for the orchestrating skill.
#[derive(Debug, Serialize)]
pub struct VerifyReport {
    pub pass: usize,
    pub fail: usize,
    pub total: usize,
    pub failures: Vec<VerifyFailure>,
}

#[derive(Debug, Serialize)]
pub struct VerifyFailure {
    pub feature_id: String,
    pub output: String,
}

impl VerifyReport {
    pub fn from_results(results: &[VerifyResult]) -> Self {
        let pass = results.iter().filter(|r| r.passed).count();
        let fail = results.len() - pass;
        let failures = results
            .iter()
            .filter(|r| !r.passed)
            .map(|r| VerifyFailure {
                feature_id: r.feature_id.clone(),
                output: r.output.clone(),
            })
            .collect();
        Self {
            pass,
            fail,
            total: results.len(),
            failures,
        }
    }

    pub fn write(&self, project_dir: &Path) -> Result<(), std::io::Error> {
        let feedback_dir = project_dir.join("feedback");
        std::fs::create_dir_all(&feedback_dir)?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(feedback_dir.join("last-verify.json"), json)?;
        Ok(())
    }
}

/// Run verify script for a single feature. Returns None if feature has no verify command.
pub fn run_verify(project_dir: &Path, verify_cmd: &str) -> Result<VerifyResult, std::io::Error> {
    let output = Command::new("bash")
        .arg("-c")
        .arg(verify_cmd)
        .current_dir(project_dir)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    Ok(VerifyResult {
        feature_id: String::new(),
        passed: output.status.success(),
        output: combined,
    })
}

/// Run all verify scripts for done/claimed features.
pub fn verify_all(project_dir: &Path) -> Result<Vec<VerifyResult>, Box<dyn std::error::Error>> {
    let features = FeatureList::load(project_dir)?;
    let mut results = Vec::new();

    for feature in &features.features {
        if feature.status == FeatureStatus::Done || feature.status == FeatureStatus::Claimed {
            let script_path = project_dir.join(&feature.verify);
            if !script_path.exists() {
                results.push(VerifyResult {
                    feature_id: feature.id.clone(),
                    passed: false,
                    output: format!("verify script not found: {}", feature.verify),
                });
                continue;
            }

            let cmd = format!("bash {}", feature.verify);
            let mut result = run_verify(project_dir, &cmd)?;
            result.feature_id = feature.id.clone();
            results.push(result);
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{Feature, FeatureList, FeatureStatus, FeatureType};

    fn make_feature(id: &str, verify: &str, status: FeatureStatus) -> Feature {
        Feature {
            id: id.into(),
            feature_type: FeatureType::Implement,
            scope: "test".into(),
            description: "test feature".into(),
            verify: verify.into(),
            depends_on: vec![],
            priority: 1,
            status,
            claimed_by: None,
            blocked_reason: None,
            context_hints: vec![],
        }
    }

    #[test]
    fn run_passing_script() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("pass.sh");
        std::fs::write(&script, "#!/bin/bash\necho PASS\nexit 0").unwrap();

        let result = run_verify(dir.path(), "bash pass.sh").unwrap();
        assert!(result.passed);
        assert!(result.output.contains("PASS"));
    }

    #[test]
    fn run_failing_script() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fail.sh");
        std::fs::write(&script, "#!/bin/bash\necho FAIL\nexit 1").unwrap();

        let result = run_verify(dir.path(), "bash fail.sh").unwrap();
        assert!(!result.passed);
        assert!(result.output.contains("FAIL"));
    }

    #[test]
    fn verify_all_runs_done_features() {
        let dir = tempfile::tempdir().unwrap();

        // Create a passing verify script
        std::fs::create_dir_all(dir.path().join("scripts/verify")).unwrap();
        std::fs::write(
            dir.path().join("scripts/verify/f001.sh"),
            "#!/bin/bash\necho ok\nexit 0",
        )
        .unwrap();

        let list = FeatureList {
            features: vec![
                make_feature("f001", "./scripts/verify/f001.sh", FeatureStatus::Done),
                make_feature(
                    "f002",
                    "./scripts/verify/f002.sh",
                    FeatureStatus::Pending,
                ),
            ],
        };
        list.save(dir.path()).unwrap();

        let results = verify_all(dir.path()).unwrap();
        // Only f001 (done) should be verified, not f002 (pending)
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].feature_id, "f001");
        assert!(results[0].passed);
    }

    #[test]
    fn write_verify_report() {
        let dir = tempfile::tempdir().unwrap();
        let results = vec![
            VerifyResult {
                feature_id: "f001".into(),
                passed: true,
                output: "ok".into(),
            },
            VerifyResult {
                feature_id: "f002".into(),
                passed: false,
                output: "left 3 != right 4".into(),
            },
        ];
        let report = VerifyReport::from_results(&results);
        assert_eq!(report.pass, 1);
        assert_eq!(report.fail, 1);
        assert_eq!(report.total, 2);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].feature_id, "f002");

        report.write(dir.path()).unwrap();
        let content = std::fs::read_to_string(dir.path().join("feedback/last-verify.json")).unwrap();
        assert!(content.contains("\"pass\": 1"));
        assert!(content.contains("\"f002\""));
    }

    #[test]
    fn verify_missing_script() {
        let dir = tempfile::tempdir().unwrap();

        let list = FeatureList {
            features: vec![make_feature(
                "f001",
                "./scripts/verify/missing.sh",
                FeatureStatus::Done,
            )],
        };
        list.save(dir.path()).unwrap();

        let results = verify_all(dir.path()).unwrap();
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        assert!(results[0].output.contains("not found"));
    }
}
