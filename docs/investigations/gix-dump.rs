//! Print everything `deltoids::git::Repo` returns for a repository as JSON,
//! time a snapshot (`bench N`) or a commit patch (`commit-bench N`), or
//! print a blob through `blob_filtered` (`filtered <hash> <path>`).
//! Build it as a binary crate depending on `deltoids` with `blob-resolve`;
//! see `gix-migration.md`.

use std::path::Path;
use std::time::Instant;
use deltoids::git::Repo;

fn esc(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn res<T>(r: Result<T, String>, f: impl Fn(T) -> String) -> String {
    match r {
        Ok(v) => format!("{{\"ok\":{}}}", f(v)),
        Err(e) => format!("{{\"err\":{}}}", esc(&e)),
    }
}

fn stages(v: Vec<deltoids::git::FileStageStatus>) -> String {
    let items: Vec<String> = v.iter().map(|s| format!(
        "{{\"path\":{},\"paths\":[{}],\"staged\":{},\"unstaged\":{}}}",
        esc(&s.path),
        s.paths.iter().map(|p| esc(p)).collect::<Vec<_>>().join(","),
        esc(&format!("{:?}", s.staged)),
        esc(&format!("{:?}", s.unstaged)),
    )).collect();
    format!("[{}]", items.join(","))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = Path::new(&args[1]);
    let repo = Repo::discover_at(dir).expect("repo");
    if args.get(2).map(String::as_str) == Some("filtered") {
        let text = repo.blob_filtered(&args[3], &args[4]).expect("filtered blob");
        print!("{text}");
        return;
    }
    if args.get(2).map(String::as_str) == Some("commit-bench") {
        let n: usize = args[3].parse().unwrap();
        let head = repo.commits("HEAD", 1).unwrap()[0].id.clone();
        let _ = repo.commit_patch(&head);
        let t = Instant::now();
        for _ in 0..n { let _ = repo.commit_patch(&head).unwrap(); }
        println!("{:.3}", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
        return;
    }
    if args.get(2).map(String::as_str) == Some("bench") {
        let n: usize = args[3].parse().unwrap();
        let _ = repo.working_tree_snapshot();
        let t = Instant::now();
        for _ in 0..n { let _ = repo.working_tree_snapshot().unwrap(); }
        println!("{:.3}", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
        return;
    }
    let mut out = Vec::new();
    out.push(format!("\"snapshot\":{}", res(repo.working_tree_snapshot(), |s| format!(
        "{{\"patch\":{},\"staged_patch\":{},\"unstaged_patch\":{},\"stages\":{}}}",
        esc(&s.patch), esc(&s.staged_patch), esc(&s.unstaged_patch), stages(s.stages)))));
    out.push(format!("\"diff\":{}", res(repo.working_tree_diff(), |s| esc(&s))));
    out.push(format!("\"status\":{}", res(repo.working_tree_status(), stages)));
    out.push(format!("\"branches\":{}", res(repo.local_branches(), |b| format!("[{}]",
        b.iter().map(|b| esc(&format!("{}|{}|{}|{}", b.name, b.head, b.summary, b.time))).collect::<Vec<_>>().join(",")))));
    let commits = repo.commits("HEAD", 50);
    out.push(format!("\"commits\":{}", res(repo.commits("HEAD", 50), |c| format!("[{}]",
        c.iter().map(|c| esc(&format!("{}|{}|{}|{}|{}|{}|{}", c.id, c.short_id, c.summary, c.message, c.author, c.email, c.time))).collect::<Vec<_>>().join(",")))));
    if let Ok(cs) = commits {
        let patches: Vec<String> = cs.iter().map(|c| res(repo.commit_patch(&c.id), |p| esc(&p))).collect();
        out.push(format!("\"commit_patches\":[{}]", patches.join(",")));
        if cs.len() > 1 {
            out.push(format!("\"after\":{}", res(repo.commits_after(&cs[cs.len()-1].id, "HEAD", 50), |c| c.len().to_string())));
            out.push(format!("\"tree_patch\":{}", res(repo.tree_patch(&cs[cs.len()-1].id, "HEAD"), |p| esc(&p))));
        }
    }
    let ign: Vec<String> = ["ignored/skipped.txt", "a.txt", "dir/untracked.txt"].iter()
        .map(|p| format!("{}", repo.is_ignored(Path::new(p)))).collect();
    out.push(format!("\"ignored\":[{}]", ign.join(",")));
    out.push(format!("\"filter\":[{},{}]", repo.has_content_filter("a.txt"), repo.has_content_filter("b.txt")));
    out.push(format!("\"cfg\":{}", esc(&format!("{:?}", repo.local_config_string("core.bare")))));
    let head = std::process::Command::new("git").args(["rev-parse", "HEAD:a.txt"]).current_dir(dir).output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_default();
    let head = head.trim();
    out.push(format!("\"blob\":{}", esc(&format!("{:?}", repo.blob_filtered(head, "a.txt")))));
    out.push(format!("\"hash_ok\":{}", deltoids::git::blob_hash_matches("hello\n", "ce013625030ba8dba906f756967f9e9ca394464a")));
    println!("{{{}}}", out.join(","));
}
