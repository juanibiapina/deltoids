use super::*;
use std::time::{Duration, Instant};

use crate::cli::browse::files::test_support::{model_of, theme};

fn epoch(width: usize, theme: &Theme) -> CacheEpoch {
    CacheEpoch {
        width,
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
    assert_eq!(block_path(cache.get(epoch, 0).unwrap()), "a.txt");
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
    assert_eq!(block_path(cache.get(epoch, 2).unwrap()), "c.txt");
    assert!(cache.get(epoch, 0).is_none());
}

#[test]
fn changing_render_settings_rejects_old_width_and_theme_results() {
    let model = model_of(&["a.txt", "b.txt", "c.txt"]);
    let mut cache = DiffCache::default();
    let initial_theme = theme();
    let initial = epoch(80, &initial_theme);
    let mut changed_theme = initial_theme.clone();
    changed_theme.syntax_theme_name = "TokyoNight".into();
    let changed = CacheEpoch {
        width: 12,
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
    assert_eq!(block_path(rows), "c.txt");
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
    assert_eq!(block_path(cache.get(epoch, 99).unwrap()), "src/f099.txt");
    assert!(
        cache.get(epoch, 50).is_none(),
        "offscreen demand should remain deferred"
    );
}

/// The file a rendered block draws, from its first line's anchor.
fn block_path(rows: &[crate::cli::browse::diff_cursor::DiffRow]) -> String {
    rows.iter()
        .find_map(|row| row.anchor.as_ref())
        .map(|anchor| anchor.path.clone())
        .unwrap_or_default()
}
