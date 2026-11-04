use super::*;

/// A change as the tests build it.
struct ChangeSet {
    files: Vec<ChangedFile>,
}

/// A file's note with "not judged" as the default, so tests compare
/// whole rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct FileNote {
    tag: Option<&'static str>,
    attention: Option<Attention>,
    low: bool,
}

fn pending_requests(changes: &ChangeSet, known: &Judgments) -> Vec<JevRequest> {
    super::pending_requests(&changes.files, known)
}

fn file_notes(changes: &ChangeSet, known: &Judgments) -> Vec<FileNote> {
    known
        .file_notes(&changes.files)
        .into_iter()
        .map(|note| {
            note.map_or_else(FileNote::default, |note| FileNote {
                tag: Some(note.tag),
                attention: note.attention,
                low: note.low,
            })
        })
        .collect()
}

fn file(path: &str, hunks: Vec<ChangedHunk>) -> ChangedFile {
    ChangedFile {
        path: path.to_string(),
        status: ChangeStatus::Modified,
        hunks,
    }
}

fn hunk(added: &[&str]) -> ChangedHunk {
    ChangedHunk {
        scope: Vec::new(),
        removed: Vec::new(),
        added: added.iter().map(|line| line.to_string()).collect(),
    }
}

fn questions(request: &JevRequest) -> Vec<String> {
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    let mut ids: Vec<String> = body["questions"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    ids.sort();
    ids
}

#[test]
fn one_request_asks_role_and_attention_for_every_hunk_but_lockfiles() {
    let changes = ChangeSet {
        files: vec![
            file("CHANGELOG.md", vec![hunk(&["- Added review order"])]),
            file("Cargo.lock", vec![hunk(&["version = \"2\""])]),
            file(
                "src/order.rs",
                vec![hunk(&["fn a() {}"]), hunk(&["fn b() {}"])],
            ),
        ],
    };

    let requests = pending_requests(&changes, &Judgments::default());

    assert_eq!(requests.len(), 1);
    assert_eq!(
        questions(&requests[0]),
        [
            "attention_h0",
            "attention_h1",
            "attention_h2",
            "breaking_h0",
            "breaking_h1",
            "breaking_h2",
            "role_h0",
            "role_h1",
            "role_h2"
        ]
    );
}

/// A hunk whose first added line starts with this is breaking (0.9);
/// with `EDGE`, just under the threshold (0.59).
const BREAK: &str = "BREAK ";
const EDGE: &str = "EDGE ";
/// A hunk whose first added line starts with this is 30% critical; with
/// `SHY`, 29%. Other hunks are fully critical when their score reaches
/// 1.5 and not critical below it.
const LEAN: &str = "LEAN ";
const SHY: &str = "SHY ";

/// A stand-in for Jev: answers every question in `request` with the
/// role and attention `judge` gives the hunk's first added line.
fn answer(request: &JevRequest, judge: impl Fn(&str) -> (&'static str, f64)) -> String {
    answer_body(&request.body, judge)
}

fn answer_body(body: &str, judge: impl Fn(&str) -> (&'static str, f64)) -> String {
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    let mut answers = serde_json::Map::new();
    for (id, hunk) in body["state"]["hunks"].as_object().unwrap() {
        let first_added = hunk["diff"]
            .as_str()
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix('+'))
            .unwrap_or("");
        let (role, attention) = judge(first_added);
        let critical = if first_added.starts_with(LEAN) {
            0.3
        } else if first_added.starts_with(SHY) {
            0.29
        } else if attention >= 1.5 {
            1.0
        } else {
            0.0
        };
        answers.insert(
            format!("role_{id}"),
            serde_json::json!({
                "type": "choice",
                "choice": role,
                "probabilities": { role: 0.9 },
                "confidence": 0.85,
            }),
        );
        answers.insert(
            format!("attention_{id}"),
            serde_json::json!({
                "type": "score",
                "score": attention,
                "legend": {},
                "probabilities": { "2": critical },
                "confidence": 0.8,
            }),
        );
        let breaking = if first_added.starts_with(BREAK) {
            0.9
        } else if first_added.starts_with(EDGE) {
            0.59
        } else {
            0.0
        };
        answers.insert(
            format!("breaking_{id}"),
            serde_json::json!({ "type": "noul", "noul": breaking }),
        );
    }
    serde_json::json!({ "model": "jev-1.13.0", "answers": answers, "usage": {} }).to_string()
}

fn judged(changes: &ChangeSet, judge: impl Fn(&str) -> (&'static str, f64)) -> Judgments {
    let mut known = Judgments::default();
    for request in pending_requests(changes, &known) {
        let response = answer(&request, &judge);
        absorb(&mut known, &request, &response).unwrap();
    }
    known
}

#[test]
fn known_judgments_are_not_asked_again_after_a_reload() {
    let before = ChangeSet {
        files: vec![file("src/a.rs", vec![hunk(&["a"])])],
    };
    let known = judged(&before, |_| ("core", 2.0));
    let after = ChangeSet {
        files: vec![
            file("src/a.rs", vec![hunk(&["a"])]),
            file("src/b.rs", vec![hunk(&["b"])]),
        ],
    };

    let requests = pending_requests(&after, &known);

    assert_eq!(requests.len(), 1);
    assert_eq!(
        questions(&requests[0]),
        ["attention_h0", "breaking_h0", "role_h0"]
    );
    assert!(requests[0].body.contains("\"diff\":\"+b\""));
}

/// A hunk of `lines` added lines of code-like text, each `width` long.
fn wide_hunk(seed: usize, lines: usize, width: usize) -> ChangedHunk {
    ChangedHunk {
        scope: vec![format!("fn scope_{seed}")],
        removed: Vec::new(),
        added: (0..lines)
            .map(|line| format!("{seed}:{line}:{}", "x(y);".repeat(width / 5)))
            .collect(),
    }
}

/// The state + longest question and whole-request sizes of `request`, in
/// characters.
fn sizes(request: &JevRequest) -> (usize, usize) {
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    let state = body["state"].to_string().len();
    let questions = body["questions"].as_object().unwrap();
    let longest = questions
        .values()
        .map(|q| q.to_string().len())
        .max()
        .unwrap();
    let all: usize = questions.values().map(|q| q.to_string().len()).sum();
    (state + longest, state + all)
}

fn covered(requests: &[JevRequest]) -> Vec<HunkKey> {
    let mut keys: Vec<HunkKey> = requests.iter().flat_map(|r| r.hunks.clone()).collect();
    keys.sort_by_key(|key| key.0);
    keys
}

fn all_keys(changes: &ChangeSet) -> Vec<HunkKey> {
    let mut keys: Vec<HunkKey> = changes
        .files
        .iter()
        .flat_map(|file| file.hunks.iter().map(|hunk| HunkKey::of(&file.path, hunk)))
        .collect();
    keys.sort_by_key(|key| key.0);
    keys
}

#[test]
fn requests_stay_within_jev_token_limits_and_cover_every_hunk_once() {
    let changes = ChangeSet {
        files: (0..40)
            .map(|f| {
                file(
                    &format!("src/module_{f}/file.rs"),
                    (0..10).map(|h| wide_hunk(f * 100 + h, 45, 240)).collect(),
                )
            })
            .collect(),
    };

    let requests = pending_requests(&changes, &Judgments::default());

    assert!(requests.len() > 1);
    for request in &requests {
        let (state_and_longest, total) = sizes(request);
        assert!(
            state_and_longest <= STATE_BUDGET_CHARS,
            "{state_and_longest}"
        );
        assert!(total <= TOTAL_BUDGET_CHARS, "{total}");
    }
    assert_eq!(covered(&requests), all_keys(&changes));
}

#[test]
fn a_huge_file_list_is_cut_to_the_files_of_each_request() {
    let changes = ChangeSet {
        files: (0..3_000)
            .map(|f| {
                file(
                    &format!("a/very/deep/directory/structure/for/file_{f}.rs"),
                    vec![hunk(&[&format!("fn f{f}() {{}}")])],
                )
            })
            .collect(),
    };

    let requests = pending_requests(&changes, &Judgments::default());

    for request in &requests {
        let (state_and_longest, total) = sizes(request);
        assert!(
            state_and_longest <= STATE_BUDGET_CHARS,
            "{state_and_longest}"
        );
        assert!(total <= TOTAL_BUDGET_CHARS, "{total}");
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(
            body["state"]["files"].as_array().unwrap().len()
                + body["state"]["other_files"].as_u64().unwrap() as usize,
            3_000
        );
    }
    assert_eq!(covered(&requests), all_keys(&changes));
}

/// A stand-in for Jev that rejects requests over `max` hunks as too
/// large and judges every hunk of the others as routine core code.
fn jev_with_limit(max: usize) -> impl Fn(&JevRequest) -> Result<String, SendError> + Sync {
    move |request: &JevRequest| {
        if request.hunks.len() > max {
            return Err(SendError::TooLarge);
        }
        Ok(answer(request, |_| ("core", 1.0)))
    }
}

#[test]
fn requests_jev_rejects_as_too_large_are_halved_until_they_fit() {
    let changes = ChangeSet {
        files: vec![file(
            "src/a.rs",
            (0..10).map(|n| hunk(&[&format!("line {n}")])).collect(),
        )],
    };
    let mut known = Judgments::default();

    let replies = send_all(&jev_with_limit(3), pending_requests(&changes, &known));
    for (request, reply) in &replies {
        absorb(&mut known, request, reply.as_ref().unwrap()).unwrap();
    }

    assert!(pending_requests(&changes, &known).is_empty());
    assert!(replies.iter().all(|(request, _)| request.hunks.len() <= 3));
}

#[test]
fn a_single_hunk_jev_rejects_as_too_large_is_reported() {
    let changes = ChangeSet {
        files: vec![file("src/a.rs", vec![hunk(&["a"]), hunk(&["b"])])],
    };

    let replies = send_all(
        &jev_with_limit(0),
        pending_requests(&changes, &Judgments::default()),
    );

    assert_eq!(replies.len(), 2);
    for (_, reply) in replies {
        assert_eq!(reply, Err("hunk too large for Jev".to_string()));
    }
}

#[test]
fn a_partial_response_keeps_the_answered_hunks() {
    let changes = ChangeSet {
        files: vec![
            file("src/a.rs", vec![hunk(&["a"])]),
            file("src/b.rs", vec![hunk(&["b"])]),
        ],
    };
    let mut known = Judgments::default();
    let request = pending_requests(&changes, &known).remove(0);
    let full: serde_json::Value = serde_json::from_str(&answer(&request, |line| match line {
        "b" => ("core", 1.8),
        _ => ("docs", 0.0),
    }))
    .unwrap();
    let mut partial = full.clone();
    let answers = partial["answers"].as_object_mut().unwrap();
    answers.remove("role_h0");

    let result = absorb(&mut known, &request, &partial.to_string());

    assert_eq!(result, Err(AbsorbError::Missing(1)));
    assert_eq!(pending_requests(&changes, &known)[0].hunks.len(), 1);
    let notes = file_notes(&changes, &known);
    assert_eq!(notes[0], FileNote::default());
    assert_eq!(notes[1].tag, Some("core"));
}

#[test]
fn an_unreadable_response_changes_nothing() {
    let changes = ChangeSet {
        files: vec![file("src/a.rs", vec![hunk(&["a"])])],
    };
    let mut known = Judgments::default();
    let request = pending_requests(&changes, &known).remove(0);

    let result = absorb(&mut known, &request, "<html>Bad gateway</html>");

    assert_eq!(result, Err(AbsorbError::Malformed));
    assert_eq!(pending_requests(&changes, &known).len(), 1);
}

#[test]
fn a_file_from_an_engine_diff_carries_changed_lines_and_scope_names() {
    let original = "fn greet() {\n    let name = \"a\";\n    println!(\"hi\");\n}\n";
    let updated = "fn greet() {\n    let name = \"b\";\n    println!(\"hi\");\n}\n";
    let diff = deltoids::Diff::compute(original, updated, "src/greet.rs");

    let changed = ChangedFile::from_diff("src/greet.rs", ChangeStatus::Modified, &diff);

    assert_eq!(changed.hunks.len(), 1);
    let hunk = &changed.hunks[0];
    assert_eq!(hunk.removed, ["    let name = \"a\";"]);
    assert_eq!(hunk.added, ["    let name = \"b\";"]);
    assert!(
        hunk.scope.iter().any(|name| name.contains("greet")),
        "scope: {:?}",
        hunk.scope
    );
}

/// A change parsed from a recorded `git show` patch. Hunks carry no
/// scope names, like a plain unified diff.
fn change_from_patch(patch: &str) -> ChangeSet {
    use deltoids::parse::{GitDiff, RawLineKind};
    let lines = |hunk: &deltoids::parse::RawHunk, kind: RawLineKind| -> Vec<String> {
        hunk.lines
            .iter()
            .filter(|line| line.kind == kind)
            .map(|line| line.content.clone())
            .collect()
    };
    ChangeSet {
        files: GitDiff::parse(patch)
            .files
            .iter()
            .map(|file| ChangedFile {
                path: file.new_path.clone(),
                status: ChangeStatus::Modified,
                hunks: file
                    .hunks
                    .iter()
                    .map(|hunk| ChangedHunk {
                        scope: Vec::new(),
                        removed: lines(hunk, RawLineKind::Removed),
                        added: lines(hunk, RawLineKind::Added),
                    })
                    .collect(),
            })
            .collect(),
    }
}

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");

/// A real POST, for recording fixtures.
fn ureq_post(post: Post<'_>) -> Result<Reply, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .post(post.url)
        .header("Authorization", post.authorization)
        .header("Content-Type", "application/json")
        .send(post.body)
        .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| error.to_string())?;
    Ok(Reply { status, body })
}

/// Record real Jev answers for every fixture patch. Run with
/// `TYPESAFE_API_KEY` set:
/// `cargo test -p deltoids-jev record_jev_fixtures -- --ignored`.
#[test]
#[ignore = "calls the TypeSafe API"]
fn record_jev_fixtures() {
    let key = std::env::var("TYPESAFE_API_KEY").expect("TYPESAFE_API_KEY");
    for name in ["79b6841", "914b576", "agent-files"] {
        let patch = std::fs::read_to_string(format!("{FIXTURES}/{name}.diff")).unwrap();
        let changes = change_from_patch(&patch);
        let requests = pending_requests(&changes, &Judgments::default());
        assert_eq!(requests.len(), 1, "{name}: one request per fixture");
        let response = transport::send(&key, &requests[0], &ureq_post).unwrap();
        std::fs::write(format!("{FIXTURES}/{name}.jev.json"), response).unwrap();
    }
}

#[test]
fn lockfile_contents_never_reach_jev() {
    let lockfiles = [
        "Cargo.lock",
        "reviewer/package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "go.sum",
        "app/gradle.lockfile",
        "ios/Package.resolved",
        "src/packages.lock.json",
        "infra/.terraform.lock.hcl",
    ];
    let mut files: Vec<ChangedFile> = lockfiles
        .iter()
        .map(|path| file(path, vec![hunk(&["LOCKFILE CONTENT"])]))
        .collect();
    files.push(file("src/a.rs", vec![hunk(&["fn a() {}"])]));
    let changes = ChangeSet { files };

    let requests = pending_requests(&changes, &Judgments::default());

    assert_eq!(
        questions(&requests[0]),
        ["attention_h0", "breaking_h0", "role_h0"]
    );
    assert!(!requests[0].body.contains("LOCKFILE CONTENT"));
}

#[test]
fn jev_chooses_among_the_current_roles() {
    let changes = ChangeSet {
        files: vec![file("src/a.rs", vec![hunk(&["fn a() {}"])])],
    };

    let request = pending_requests(&changes, &Judgments::default()).remove(0);
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    let roles: Vec<&String> = body["questions"]["role_h0"]["criteria"]
        .as_object()
        .unwrap()
        .keys()
        .collect();

    for added in ["comments", "config", "deps", "removal", "ci", "agents"] {
        assert!(roles.iter().any(|role| *role == added), "{roles:?}");
    }
    for removed in ["support", "mechanical", "adapt", "changelog", "public-api"] {
        assert!(!roles.iter().any(|role| *role == removed), "{roles:?}");
    }
}

#[test]
fn file_notes_tag_each_file_by_its_best_judged_hunk() {
    let changes = ChangeSet {
        files: vec![
            file("src/engine.rs", vec![hunk(&["helper"]), hunk(&["engine"])]),
            file("src/plain.rs", vec![hunk(&["plain"])]),
            file("CHANGELOG.md", vec![hunk(&["- Added"])]),
            file("Cargo.lock", vec![hunk(&["version"])]),
            file("tests/order.rs", vec![hunk(&["#[test]"])]),
            file("docs/guide.md", vec![hunk(&["Press f"])]),
            file("src/commented.rs", vec![hunk(&["/// Explains a."])]),
            file("Cargo.toml", vec![hunk(&["ureq"])]),
            file("src/uses.rs", vec![hunk(&["use std::fmt;"])]),
            file("config.toml", vec![hunk(&["retries = 5"])]),
            file("ci.yml", vec![hunk(&["run: cargo test"])]),
            file("src/old.rs", vec![hunk(&["fn gone() {}"])]),
            file("web/app.min.js", vec![hunk(&["minified"])]),
            file("AGENTS.md", vec![hunk(&["Run cargo fmt"])]),
        ],
    };
    let known = judged(&changes, |line| match line {
        "engine" => ("core", 1.8),
        "helper" => ("core", 0.2),
        "- Added" => ("docs", 0.3),
        "use std::fmt;" => ("imports", 0.1),
        "#[test]" => ("test", 0.2),
        "Press f" => ("docs", 0.2),
        "/// Explains a." => ("comments", 0.1),
        "ureq" => ("deps", 1.0),
        "retries = 5" => ("config", 1.2),
        "run: cargo test" => ("build", 0.3),
        "fn gone() {}" => ("removal", 0.3),
        "Run cargo fmt" => ("agents", 0.6),
        _ => ("refactor", 0.7),
    });
    let notes = file_notes(&changes, &known);
    let summary: Vec<(Option<&str>, Option<Attention>, bool)> = notes
        .iter()
        .map(|note| (note.tag, note.attention, note.low))
        .collect();

    assert_eq!(
        summary,
        [
            (Some("core"), Some(Attention::Critical), false),
            (Some("refactor"), Some(Attention::Careful), false),
            (Some("docs"), Some(Attention::Straightforward), false),
            (Some("lockfile"), None, true),
            (Some("test"), Some(Attention::Straightforward), true),
            (Some("docs"), Some(Attention::Straightforward), false),
            (Some("comments"), Some(Attention::Straightforward), true),
            (Some("deps"), Some(Attention::Careful), false),
            (Some("imports"), Some(Attention::Straightforward), true),
            (Some("config"), Some(Attention::Careful), false),
            (Some("build"), Some(Attention::Straightforward), false),
            (Some("removal"), Some(Attention::Straightforward), false),
            (None, None, false),
            (Some("agents"), Some(Attention::Careful), false),
        ]
    );

    let without_jev = file_notes(&changes, &Judgments::default());
    for (index, note) in without_jev.iter().enumerate() {
        if changes.files[index].path == "Cargo.lock" {
            assert_eq!(note.tag, Some("lockfile"));
        } else {
            assert_eq!(*note, FileNote::default());
        }
    }
}

#[test]
fn every_file_but_lockfiles_and_generated_files_reaches_jev() {
    let changes = ChangeSet {
        files: vec![
            file("CHANGELOG.md", vec![hunk(&["- Added guidance"])]),
            file("README.md", vec![hunk(&["Press f"])]),
            file("AGENTS.md", vec![hunk(&["Rules"])]),
            file("src/fixtures/case.diff", vec![hunk(&["+fn a() {}"])]),
            file("Cargo.lock", vec![hunk(&["LOCKFILE CONTENT"])]),
            file("app.min.js", vec![hunk(&["MINIFIED CONTENT"])]),
        ],
    };

    let request = pending_requests(&changes, &Judgments::default()).remove(0);

    assert_eq!(request.hunks.len(), 4);
    for line in ["- Added guidance", "Press f", "Rules", "+fn a() {}"] {
        assert!(request.body.contains(line), "{line} missing");
    }
    assert!(!request.body.contains("CONTENT"));
}

#[test]
fn a_file_shows_the_hunk_that_needs_the_most_attention() {
    let changes = ChangeSet {
        files: vec![file(
            "src/a.rs",
            vec![
                hunk(&["fn a() {}"]),
                hunk(&["#[test]"]),
                hunk(&["fn b() {}"]),
            ],
        )],
    };
    let known = judged(&changes, |line| match line {
        "#[test]" => ("test", 1.8),
        "fn a() {}" => ("core", 0.6),
        _ => ("refactor", 1.8),
    });

    let note = file_notes(&changes, &known)[0];

    assert_eq!(note.tag, Some("test"));
    assert_eq!(note.attention, Some(Attention::Critical));
}

/// Judgments for a recorded `git show` patch, from its recorded Jev
/// answers.
fn recorded_notes(name: &str) -> Vec<(String, Option<&'static str>)> {
    let patch = std::fs::read_to_string(format!("{FIXTURES}/{name}.diff")).unwrap();
    let response = std::fs::read_to_string(format!("{FIXTURES}/{name}.jev.json")).unwrap();
    let changes = change_from_patch(&patch);
    let mut known = Judgments::default();
    let request = pending_requests(&changes, &known).remove(0);
    absorb(&mut known, &request, &response).unwrap();
    changes
        .files
        .iter()
        .zip(file_notes(&changes, &known))
        .map(|(file, note)| (file.path.clone(), note.tag))
        .collect()
}

/// The signal fix's three terminal hunks score 1.47 to 1.54; Jev gives
/// each about even odds of critical.
#[test]
fn recorded_answers_mark_every_hunk_jev_leans_critical_on() {
    let patch = std::fs::read_to_string(format!("{FIXTURES}/79b6841.diff")).unwrap();
    let response = std::fs::read_to_string(format!("{FIXTURES}/79b6841.jev.json")).unwrap();
    let changes = change_from_patch(&patch);
    let mut known = Judgments::default();
    let request = pending_requests(&changes, &known).remove(0);
    absorb(&mut known, &request, &response).unwrap();

    let critical = known
        .0
        .values()
        .filter(|judgment| judgment.level() == Attention::Critical)
        .count();

    assert_eq!(critical, 3);
}

#[test]
fn recorded_answers_tag_the_signal_fix() {
    let notes = recorded_notes("79b6841");

    assert!(
        notes.contains(&("CHANGELOG.md".into(), Some("docs"))),
        "{notes:?}"
    );
    assert!(
        notes.contains(&("crates/deltoids-cli/Cargo.toml".into(), Some("deps"))),
        "{notes:?}"
    );
    assert!(
        notes.contains(&("Cargo.lock".into(), Some("lockfile"))),
        "{notes:?}"
    );
}

#[test]
fn recorded_answers_tag_the_stage_all_tests() {
    let notes = recorded_notes("914b576");

    assert!(
        notes.contains(&(
            "crates/deltoids-cli/src/cli/browse/files/actions/tests.rs".into(),
            Some("test")
        )),
        "{notes:?}"
    );
}

#[test]
fn recorded_answers_tag_agent_files_apart_from_docs() {
    let notes = recorded_notes("agent-files");

    for path in [
        "AGENTS.md",
        "CLAUDE.md",
        ".agents/skills/review/SKILL.md",
        ".github/prompts/release.prompt.md",
        ".cursor/rules/style.mdc",
    ] {
        assert!(
            notes.contains(&(path.into(), Some("agents"))),
            "{path}: {notes:?}"
        );
    }
    assert!(
        notes.contains(&("README.md".into(), Some("docs"))),
        "{notes:?}"
    );
}

#[test]
fn every_role_question_names_the_hunk_file_path() {
    let changes = ChangeSet {
        files: vec![file(
            "src/judgments/fixtures/case.diff",
            vec![hunk(&["- CHANGELOG line"])],
        )],
    };

    let request = pending_requests(&changes, &Judgments::default()).remove(0);
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();

    assert_eq!(
        body["state"]["hunks"]["h0"]["file"],
        "src/judgments/fixtures/case.diff"
    );
    let instructions = body["questions"]["role_h0"]["instructions"]
        .as_str()
        .unwrap();
    assert!(
        instructions.contains("to the file src/judgments/fixtures/case.diff"),
        "{instructions}"
    );
}

#[test]
fn the_transport_tells_too_large_requests_from_other_failures() {
    assert_eq!(
        transport::outcome(Reply {
            status: 400,
            body: r#"{"detail":{"error_type":"max_tokens_exceeded"}}"#.into()
        }),
        Err(SendError::TooLarge)
    );
    assert_eq!(
        transport::outcome(Reply {
            status: 401,
            body: String::new()
        }),
        Err(SendError::Failed("Jev rejected the API key".into()))
    );
    assert_eq!(
        transport::outcome(Reply {
            status: 200,
            body: "{}".into()
        }),
        Ok("{}".to_string())
    );
}

#[test]
fn asking_posts_to_jev_with_the_key_and_records_its_answers() {
    let files = vec![
        file("src/a.rs", vec![hunk(&["fn a() {}"])]),
        file("tests/a.rs", vec![hunk(&["#[test]"])]),
        file("Cargo.lock", vec![hunk(&["version"])]),
    ];
    let mut known = Judgments::default();
    let post = |post: Post<'_>| {
        assert_eq!(post.url, "https://api.typesafe.ai/v1/systemone");
        assert_eq!(post.authorization, "Bearer k");
        let body = answer_body(post.body, |line| match line {
            "#[test]" => ("test", 0.1),
            _ => ("core", 1.0),
        });
        Ok(Reply { status: 200, body })
    };

    let answers = known.pending(&files).unwrap().ask("k", &post);

    assert_eq!(known.add(answers), Ok(()));
    assert!(known.pending(&files).is_none());
    assert_eq!(
        known.file_notes(&files),
        [
            Some(Note {
                tag: "core",
                attention: Some(Attention::Careful),
                low: false
            }),
            Some(Note {
                tag: "test",
                attention: Some(Attention::Straightforward),
                low: true
            }),
            Some(Note {
                tag: "lockfile",
                attention: None,
                low: true
            }),
        ]
    );
    assert_eq!(
        known.note("tests/a.rs", &hunk(&["#[test]"])).map(|n| n.tag),
        Some("test")
    );
}

#[test]
fn a_busy_jev_is_asked_again() {
    let files = vec![file("src/a.rs", vec![hunk(&["fn a() {}"])])];
    let mut known = Judgments::default();
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let post = |post: Post<'_>| {
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            return Ok(Reply {
                status: 429,
                body: String::new(),
            });
        }
        let body = answer_body(post.body, |_| ("core", 1.0));
        Ok(Reply { status: 200, body })
    };

    let answers = known.pending(&files).unwrap().ask("k", &post);

    assert_eq!(known.add(answers), Ok(()));
    assert_eq!(calls.into_inner(), 2);
}

#[test]
fn a_rejected_key_is_reported_and_leaves_every_hunk_pending() {
    let files = vec![file("src/a.rs", vec![hunk(&["fn a() {}"])])];
    let mut known = Judgments::default();
    let post = |_: Post<'_>| {
        Ok(Reply {
            status: 401,
            body: String::new(),
        })
    };

    let answers = known.pending(&files).unwrap().ask("k", &post);

    assert_eq!(known.add(answers), Err("Jev rejected the API key".into()));
    assert!(known.pending(&files).is_some());
    assert_eq!(known.file_notes(&files), [None]);
}

#[test]
fn a_breaking_hunk_is_critical_and_tags_its_file() {
    let changes = ChangeSet {
        files: vec![
            file(
                "src/api.rs",
                vec![hunk(&["fn core() {}"]), hunk(&["BREAK pub fn gone() {}"])],
            ),
            file("src/edge.rs", vec![hunk(&["EDGE pub fn maybe() {}"])]),
        ],
    };
    let known = judged(&changes, |line| match line {
        "fn core() {}" => ("core", 1.3),
        _ => ("removal", 1.0),
    });

    let notes = file_notes(&changes, &known);

    assert_eq!(
        (notes[0].tag, notes[0].attention),
        (Some("removal"), Some(Attention::Critical))
    );
    assert_eq!(notes[1].attention, Some(Attention::Careful));
}

#[test]
fn a_hunk_leaning_critical_is_critical_below_the_critical_score() {
    let changes = ChangeSet {
        files: vec![
            file("src/lean.rs", vec![hunk(&["LEAN fn a() {}"])]),
            file("src/shy.rs", vec![hunk(&["SHY fn b() {}"])]),
        ],
    };
    let known = judged(&changes, |_| ("core", 1.2));

    let levels: Vec<_> = file_notes(&changes, &known)
        .iter()
        .map(|note| note.attention)
        .collect();

    assert_eq!(
        levels,
        [Some(Attention::Critical), Some(Attention::Careful)]
    );
}

#[test]
fn an_attention_answer_without_the_critical_probability_is_missing() {
    let changes = ChangeSet {
        files: vec![file("src/a.rs", vec![hunk(&["a"])])],
    };
    let mut known = Judgments::default();
    let request = pending_requests(&changes, &known).remove(0);
    let mut answers: serde_json::Value =
        serde_json::from_str(&answer(&request, |_| ("core", 1.0))).unwrap();
    answers["answers"]["attention_h0"]["probabilities"] = serde_json::json!({});

    let result = absorb(&mut known, &request, &answers.to_string());

    assert_eq!(result, Err(AbsorbError::Missing(1)));
}

#[test]
fn an_answer_without_the_breaking_question_is_missing() {
    let changes = ChangeSet {
        files: vec![file("src/a.rs", vec![hunk(&["a"])])],
    };
    let mut known = Judgments::default();
    let request = pending_requests(&changes, &known).remove(0);
    let mut answers: serde_json::Value =
        serde_json::from_str(&answer(&request, |_| ("core", 1.0))).unwrap();
    answers["answers"]
        .as_object_mut()
        .unwrap()
        .remove("breaking_h0");

    let result = absorb(&mut known, &request, &answers.to_string());

    assert_eq!(result, Err(AbsorbError::Missing(1)));
}

#[test]
fn the_attention_question_names_the_file_and_says_test_data_needs_a_skim() {
    let changes = ChangeSet {
        files: vec![file(
            "src/judgments/fixtures/case.diff",
            vec![hunk(&["+fn a() {}"])],
        )],
    };

    let request = pending_requests(&changes, &Judgments::default()).remove(0);
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    let instructions = body["questions"]["attention_h0"]["instructions"]
        .as_str()
        .unwrap();

    assert!(
        instructions.contains("to the file src/judgments/fixtures/case.diff"),
        "{instructions}"
    );
    assert!(instructions.contains("needs only a skim"), "{instructions}");
}

#[test]
fn the_breaking_question_names_the_file() {
    let changes = ChangeSet {
        files: vec![file("crates/cli/src/main.rs", vec![hunk(&["fn a() {}"])])],
    };

    let request = pending_requests(&changes, &Judgments::default()).remove(0);
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    let instructions = body["questions"]["breaking_h0"]["instructions"]
        .as_str()
        .unwrap();

    assert!(
        instructions.contains("to the file crates/cli/src/main.rs"),
        "{instructions}"
    );
}

#[test]
fn attention_has_three_levels() {
    let changes = ChangeSet {
        files: vec![file("src/a.rs", vec![hunk(&["fn a() {}"])])],
    };

    let request = pending_requests(&changes, &Judgments::default()).remove(0);
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    let levels: Vec<&str> = body["questions"]["attention_h0"]["criteria"]
        .as_array()
        .unwrap()
        .iter()
        .map(|level| level.as_str().unwrap().split(':').next().unwrap())
        .collect();

    assert_eq!(levels, ["Straightforward", "Careful", "Critical"]);
}
