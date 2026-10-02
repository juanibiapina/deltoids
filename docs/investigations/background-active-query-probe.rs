use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("temporary repository path");
    let repo = deltoids::git::Repo::discover_at(std::path::Path::new(&path)).unwrap();
    let iterations = 20;
    repo.working_tree_diff().unwrap();
    repo.working_tree_status().unwrap();
    let started = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(repo.working_tree_diff().unwrap());
    }
    let diff_ms = started.elapsed().as_secs_f64() * 1000.0 / f64::from(iterations);
    let started = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(repo.working_tree_status().unwrap());
    }
    let status_ms = started.elapsed().as_secs_f64() * 1000.0 / f64::from(iterations);
    repo.working_tree_snapshot().unwrap();
    let started = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(repo.working_tree_snapshot().unwrap());
    }
    let snapshot_ms = started.elapsed().as_secs_f64() * 1000.0 / f64::from(iterations);
    println!(
        "{{\"iterations\":{iterations},\"diff_ms_per_call\":{diff_ms:.2},\"status_ms_per_call\":{status_ms:.2},\"combined_ms_per_refresh\":{:.2},\"snapshot_ms_per_call\":{snapshot_ms:.2},\"reduction_percent\":{:.2}}}",
        diff_ms + status_ms,
        100.0 * (diff_ms + status_ms - snapshot_ms) / (diff_ms + status_ms)
    );
}
