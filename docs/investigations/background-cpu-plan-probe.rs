use std::{fs, path::{Path, PathBuf}, process::Command, sync::{Arc, Mutex, mpsc}, time::{Duration, Instant}};
use deltoids_cli::TraceStore;
use notify::{Event, EventKind, event::Flag};

#[path = "../../crates/deltoids-cli/src/cli/browse/watch.rs"]
mod watch;

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}
fn entry(cwd: &str, stamp: &str, id: &str) -> String {
    format!(r#"{{"v":1,"tool":"write","traceId":"{id}","timestamp":"{stamp}","cwd":"{cwd}","path":"f","reason":"probe","ok":true,"content":"x"}}"#)
}
fn main() {
    let root = PathBuf::from(std::env::args().nth(1).unwrap());
    fs::create_dir_all(&root).unwrap();
    let repo_dir = root.join("repo");
    fs::create_dir_all(&repo_dir).unwrap();
    git(&repo_dir, &["init"]);
    fs::write(repo_dir.join("f"), "before\n").unwrap();
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["-c", "user.name=Probe", "-c", "user.email=probe@example.com", "commit", "-m", "initial"]);
    fs::write(repo_dir.join("f"), "after\n").unwrap();
    let repo = deltoids::git::Repo::discover_at(&repo_dir).unwrap();
    let patch_before = repo.working_tree_diff().unwrap();
    let status_before = repo.working_tree_status().unwrap();
    git(&repo_dir, &["add", "f"]);
    let patch_after = repo.working_tree_diff().unwrap();
    let status_after = repo.working_tree_status().unwrap();
    assert_eq!(patch_before, patch_after);
    assert_ne!(status_before, status_after);
    println!("staging: same patch, changed status {:?} -> {:?}", status_before, status_after);

    let linked_path = root.join("linked");
    git(&repo_dir, &["worktree", "add", "-b", "probe-linked", linked_path.to_str().unwrap()]);
    let main_git = git2::Repository::open(&repo_dir).unwrap();
    let linked_git = git2::Repository::open(&linked_path).unwrap();
    assert_ne!(linked_git.path(), main_git.path());
    assert_eq!(linked_git.commondir(), main_git.commondir());
    assert_eq!(linked_git.head().unwrap().target(), main_git.head().unwrap().target());
    println!("linked worktree: separate Git directory, shared common directory, HEAD resolves");

    let rescan = Event::new(EventKind::Other).set_flag(Flag::Rescan);
    assert!(rescan.need_rescan());
    assert!(!watch::path_warrants_reload(&repo, &rescan.paths));
    println!("rescan: need_rescan=true, paths={:?}, existing reload filter=false", rescan.paths);
    // A bounded callback design: coalesce first, then nonblocking wakeup.
    let pending = Arc::new(Mutex::new(false));
    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    for _ in 0..100_000 {
        *pending.lock().unwrap() |= rescan.need_rescan();
        let _ = wake_tx.try_send(());
    }
    assert_eq!(wake_rx.try_iter().count(), 1);
    assert!(*pending.lock().unwrap());
    println!("coalescing: 100000 callbacks -> one wakeup, retained rescan=true");

    let store_root = root.join("traces");
    let store = TraceStore::with_root(store_root.clone());
    let a = store_root.join("a");
    let b = store_root.join("b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    fs::write(a.join("entries.jsonl"), format!("{}\n", entry("local", "2026-10-01T00:00:01Z", "a"))).unwrap();
    fs::write(b.join("entries.jsonl"), format!("{}\n", entry("local", "2026-10-01T00:00:02Z", "b"))).unwrap();
    let before = store.list_for_cwd("local").unwrap();
    store.append("a", &serde_json::from_str::<serde_json::Value>(&entry("local", "2026-10-01T00:00:03Z", "a")).unwrap()).unwrap();
    let after = store.list_for_cwd("local").unwrap();
    assert_ne!(before[0].trace_id, after[0].trace_id);
    println!("trace index 0: {} -> {} after append to existing trace", before[0].trace_id, after[0].trace_id);

    let partial = store_root.join("partial");
    fs::create_dir_all(&partial).unwrap();
    let full = entry("local", "2026-10-01T00:00:04Z", "partial");
    fs::write(partial.join("entries.jsonl"), &full[..full.len()/2]).unwrap();
    assert!(store.read("partial").is_err());
    fs::write(partial.join("entries.jsonl"), &full).unwrap();
    assert_eq!(store.read("partial").unwrap().len(), 1);
    println!("trace parse: partial record errors, complete record without newline is accepted");

    let hidden_since = Instant::now() - Duration::from_secs(1);
    let timeout = Duration::from_millis(200).saturating_sub(hidden_since.elapsed());
    assert!(timeout.is_zero());
    println!("hidden pending deadline: existing timeout expression is zero after debounce expires");

    let mut focus = Vec::new();
    crossterm::execute!(&mut focus, crossterm::event::EnableFocusChange, crossterm::event::DisableFocusChange).unwrap();
    assert_eq!(focus, b"\x1b[?1004h\x1b[?1004l");
    println!("focus commands: {:?}", String::from_utf8_lossy(&focus));
}
