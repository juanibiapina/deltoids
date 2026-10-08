use super::*;

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

/// A stand-in for Jev: answers every question in `request` with the
/// role and attention `judge` gives the hunk's first added line.
fn answer(request: &JevRequest, judge: impl Fn(&str) -> (&'static str, f64)) -> String {
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    let mut answers = serde_json::Map::new();
    for (id, hunk) in body["state"]["hunks"].as_object().unwrap() {
        let first_added = hunk["diff"]
            .as_str()
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix('+'))
            .unwrap_or("");
        let (role, attention) = judge(first_added);
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
                "probabilities": {},
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

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/judgments/fixtures");

/// Record real Jev answers for every fixture patch. Run with
/// `TYPESAFE_API_KEY` set:
/// `cargo test -p deltoids-cli --lib record_jev_fixtures -- --ignored`.
#[test]
#[ignore = "calls the TypeSafe API"]
fn record_jev_fixtures() {
    let key = jev::key_from_env().expect("TYPESAFE_API_KEY");
    for name in ["79b6841", "914b576"] {
        let patch = std::fs::read_to_string(format!("{FIXTURES}/{name}.diff")).unwrap();
        let changes = change_from_patch(&patch);
        let requests = pending_requests(&changes, &Judgments::default());
        assert_eq!(requests.len(), 1, "{name}: one request per fixture");
        let response = jev::send(&key, &requests[0]).unwrap();
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

    for added in ["comments", "config", "deps", "removal", "ci"] {
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
        jev::outcome(
            400,
            r#"{"detail":{"error_type":"max_tokens_exceeded"}}"#.into()
        ),
        Err(SendError::TooLarge)
    );
    assert_eq!(
        jev::outcome(401, String::new()),
        Err(SendError::Failed(
            "Jev rejected the key in TYPESAFE_API_KEY".into()
        ))
    );
    assert_eq!(jev::outcome(200, "{}".into()), Ok("{}".to_string()));
}

#[test]
fn a_breaking_hunk_marks_its_file_even_when_another_hunk_sets_the_tag() {
    let changes = ChangeSet {
        files: vec![
            file(
                "src/api.rs",
                vec![hunk(&["BREAK pub fn gone() {}"]), hunk(&["fn core() {}"])],
            ),
            file("src/edge.rs", vec![hunk(&["EDGE pub fn maybe() {}"])]),
            file("src/plain.rs", vec![hunk(&["fn plain() {}"])]),
        ],
    };
    let known = judged(&changes, |line| match line {
        "fn core() {}" => ("core", 1.8),
        _ => ("refactor", 1.0),
    });

    let notes = file_notes(&changes, &known);

    assert_eq!((notes[0].tag, notes[0].breaking), (Some("core"), true));
    assert!(!notes[1].breaking);
    assert!(!notes[2].breaking);
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
