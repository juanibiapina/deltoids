use super::*;
use std::time::{Duration, Instant};

use crate::cli::browse::files::test_support::{line_text, model_of, theme};
use deltoids::ChangeLayout;

fn epoch(width: usize, theme: &Theme) -> CacheEpoch {
    CacheEpoch {
        width,
        layout: ChangeLayout::Grouped,
        syntax_theme: deltoids::theme_name_key(&theme.syntax_theme_name),
    }
}

fn finish(cache: &mut DiffCache) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while cache.pending() {
        cache.collect();
        assert!(Instant::now() < deadline, "renderer did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn retention_pressure_keeps_the_selected_file_and_stops_when_preparation_finishes() {
    let model = model_of(&["a.txt", "b.txt", "c.txt"]);
    let mut cache = DiffCache {
        budget: Budget(1),
        ..DiffCache::default()
    };
    let theme = theme();
    let epoch = epoch(80, &theme);
    cache.request(
        epoch,
        &model,
        Column::Unstaged,
        &[0, 1, 2],
        Some(0..1),
        None,
        &theme,
    );
    finish(&mut cache);
    assert_eq!(line_text(&cache.get(epoch, 0).unwrap()[0].line), "a.txt");
    assert!(cache.get(epoch, 2).is_none());
    cache.request(
        epoch,
        &model,
        Column::Unstaged,
        &[0, 1, 2],
        Some(0..1),
        None,
        &theme,
    );
    assert!(!cache.pending(), "evicted preparation must not run forever");

    cache.request(
        epoch,
        &model,
        Column::Unstaged,
        &[0, 1, 2],
        Some(2..3),
        None,
        &theme,
    );
    finish(&mut cache);
    assert_eq!(line_text(&cache.get(epoch, 2).unwrap()[0].line), "c.txt");
    assert!(cache.get(epoch, 0).is_none());
}

#[test]
fn changing_render_settings_rejects_old_width_layout_and_theme_results() {
    let model = model_of(&["a.rs", "b.rs", "c.rs"]);
    let mut cache = DiffCache::default();
    let initial_theme = theme();
    let initial = epoch(80, &initial_theme);
    let mut changed_theme = initial_theme.clone();
    changed_theme.syntax_theme_name = "TokyoNight".into();
    let changed = CacheEpoch {
        width: 12,
        layout: ChangeLayout::Interleaved {
            group: std::num::NonZeroUsize::new(1).unwrap(),
        },
        syntax_theme: "TokyoNight",
    };
    cache.request(
        initial,
        &model,
        Column::Unstaged,
        &[0, 1, 2],
        Some(0..1),
        None,
        &initial_theme,
    );
    cache.request(
        changed,
        &model,
        Column::Unstaged,
        &[0, 1, 2],
        Some(2..3),
        None,
        &changed_theme,
    );
    finish(&mut cache);
    assert!(cache.get(initial, 0).is_none());
    let rows = cache.get(changed, 2).unwrap();
    assert_eq!(line_text(&rows[0].line), "c.rs");
    assert!(rows.iter().all(|row| row.line.width() <= changed.width));
}

#[test]
fn a_directory_viewport_can_render_its_last_file_before_the_directory_finishes() {
    let paths: Vec<_> = (0..100)
        .map(|index| format!("src/f{index:03}.txt"))
        .collect();
    let refs: Vec<_> = paths.iter().map(String::as_str).collect();
    let model = model_of(&refs);
    let order: Vec<_> = (0..100).collect();
    let theme = theme();
    let epoch = epoch(80, &theme);
    let mut cache = DiffCache::default();
    cache.request(
        epoch,
        &model,
        Column::Unstaged,
        &order,
        Some(0..100),
        None,
        &theme,
    );
    cache.request(
        epoch,
        &model,
        Column::Unstaged,
        &order,
        Some(0..100),
        Some(99),
        &theme,
    );
    finish(&mut cache);
    assert_eq!(
        line_text(&cache.get(epoch, 99).unwrap()[0].line),
        "src/f099.txt"
    );
    assert!(
        cache.get(epoch, 50).is_none(),
        "offscreen demand should remain deferred"
    );
}
