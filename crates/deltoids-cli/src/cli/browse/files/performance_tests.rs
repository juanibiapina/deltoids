use super::model::build_model;
use super::sidebar_pane::build_sidebar;
use super::test_support::theme;
use crate::sidebar::{ChangeKind, StageStatus};
use std::collections::HashMap;
use std::time::Instant;

#[test]
#[ignore = "manual Files scaling probe"]
fn files_scaling_probe() {
    for count in [1000, 4000] {
        let mut patch = String::new();
        let mut stages = HashMap::new();
        for index in 0..count {
            let path = format!("file{index:05}.bin");
            patch.push_str(&format!(
                "diff --git a/{path} b/{path}\nBinary files a/{path} and b/{path} differ\n"
            ));
            stages.insert(
                path,
                StageStatus {
                    staged: None,
                    unstaged: Some(ChangeKind::Modified),
                },
            );
        }
        let started = Instant::now();
        let model = build_model(&patch, None, stages).unwrap();
        let build = started.elapsed();
        assert_eq!(model.files.len(), count);
        let mut sidebar = build_sidebar(&model, &theme());
        let started = Instant::now();
        for _ in 0..100 {
            sidebar.move_down(20);
        }
        eprintln!(
            "{count} files: model={build:?}, 100 navigation keys={:?}",
            started.elapsed()
        );
    }
}
