use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::features::FeatureList;
use crate::verify;

/// Background orchestration results shared with the TUI.
pub struct OrchestrationUpdate {
    pub verify_results: Vec<verify::VerifyResult>,
    pub reopened: Vec<String>,
    pub all_done: bool,
}

/// Run background orchestration: poll features.json, run verify on done features,
/// reopen failed features. Returns when all features are done or stop is signaled.
pub async fn run_orchestration(
    project_dir: &Path,
    stop: Arc<AtomicBool>,
    on_update: impl Fn(OrchestrationUpdate) + Send + 'static,
) {
    let project_dir = project_dir.to_path_buf();

    tokio::spawn(async move {
        let mut last_done_count = 0usize;

        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }

            tokio::time::sleep(Duration::from_secs(2)).await;

            // Load current feature state
            let features = match FeatureList::load(&project_dir) {
                Ok(f) => f,
                Err(_) => continue,
            };

            let counts = features.status_counts();
            let current_done = counts.done;

            // If new features were marked done since last check, run verify
            if current_done > last_done_count {
                last_done_count = current_done;

                let verify_results = match verify::verify_all(&project_dir) {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                // Write verify report
                let report = verify::VerifyReport::from_results(&verify_results);
                let _ = report.write(&project_dir);

                // Reopen features that failed verify
                let mut reopened = Vec::new();
                if let Ok(mut features) = FeatureList::load(&project_dir) {
                    for result in &verify_results {
                        if !result.passed {
                            if features.reopen(&result.feature_id).is_ok() {
                                reopened.push(result.feature_id.clone());
                            }
                        }
                    }
                    if !reopened.is_empty() {
                        let _ = features.save(&project_dir);
                    }
                }

                let all_done = FeatureList::load(&project_dir)
                    .map(|f| f.all_done())
                    .unwrap_or(false);

                on_update(OrchestrationUpdate {
                    verify_results,
                    reopened,
                    all_done,
                });

                if all_done {
                    break;
                }
            }

            // Also check if all done without new completions
            if features.all_done() {
                on_update(OrchestrationUpdate {
                    verify_results: vec![],
                    reopened: vec![],
                    all_done: true,
                });
                break;
            }
        }
    });
}
