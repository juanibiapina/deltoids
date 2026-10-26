//! What `DefinitionDiff::compute` reports for each kind of change, in each
//! language: one line per changed definition, nested definitions indented
//! under their parent.

use deltoids::{DefinitionChange, DefinitionDiff, Status};

/// The report as text: `status  kind  name  [aspects]  old A-B  new C-D
/// +added -removed`, then the signature that was added, removed, or
/// changed.
fn report(path: &str, before: &str, after: &str) -> String {
    let diff = DefinitionDiff::compute(before, after, path).expect("a supported language");
    let mut out = String::new();
    if diff.has_errors {
        out.push_str("syntax errors\n");
    }
    for change in &diff.definitions {
        write_change(&mut out, change, 0);
    }
    for top in &diff.top_level {
        out.push_str(&format!(
            "top-level  {}  at {}\n",
            aspects(&top.aspects),
            top.at
        ));
    }
    out
}

fn aspects(aspects: &deltoids::Aspects) -> String {
    let names: Vec<&str> = [
        (aspects.signature, "signature"),
        (aspects.attributes, "attributes"),
        (aspects.body, "body"),
        (aspects.docs, "docs"),
        (aspects.comments, "comments"),
        (aspects.formatting, "formatting"),
        (aspects.imports, "imports"),
    ]
    .into_iter()
    .filter(|(set, _)| *set)
    .map(|(_, name)| name)
    .collect();
    format!("[{}]", names.join(", "))
}

fn write_change(out: &mut String, change: &DefinitionChange, depth: usize) {
    if !change.is_changed() {
        return;
    }
    let indent = "  ".repeat(depth);
    let status = match &change.status {
        Status::Added => "added".to_string(),
        Status::Removed => "removed".to_string(),
        Status::Modified => "modified".to_string(),
        Status::Renamed { from } => format!("renamed from {from}:"),
        Status::Moved { from: Some(from) } if from.is_empty() => {
            "moved from top level:".to_string()
        }
        Status::Moved { from: Some(from) } => format!("moved from {}:", from.join(" › ")),
        Status::Moved { from: None } => "moved:".to_string(),
        Status::Container => "in".to_string(),
        Status::Unchanged => unreachable!("skipped above"),
    };
    let mut line = format!("{indent}{status}  {}  {}", change.kind.label(), change.name);
    if !change.aspects.is_empty() {
        line.push_str(&format!("  {}", aspects(&change.aspects)));
    }
    let lines = |place: &Option<deltoids::Place>| {
        place
            .as_ref()
            .map(|place| format!("{}-{}", place.lines.start(), place.lines.end()))
    };
    if let Some(old) = lines(&change.old) {
        line.push_str(&format!("  old {old}"));
    }
    if let Some(new) = lines(&change.new) {
        line.push_str(&format!("  new {new}"));
    }
    let (added, removed) = change.lines;
    if added + removed > 0 {
        line.push_str(&format!("  +{added} -{removed}"));
    }
    if change.uncertain {
        line.push_str("  uncertain");
    }
    out.push_str(&line);
    out.push('\n');
    let signature = |place: &Option<deltoids::Place>| {
        place
            .as_ref()
            .map(|place| place.signature.clone())
            .unwrap_or_default()
    };
    match change.status {
        Status::Added => out.push_str(&format!("{indent}  sig+ {}\n", signature(&change.new))),
        Status::Removed => out.push_str(&format!("{indent}  sig- {}\n", signature(&change.old))),
        _ if change.aspects.signature => {
            out.push_str(&format!("{indent}  sig- {}\n", signature(&change.old)));
            out.push_str(&format!("{indent}  sig+ {}\n", signature(&change.new)));
        }
        _ => {}
    }
    if !matches!(change.status, Status::Added | Status::Removed) {
        for child in &change.children {
            write_change(out, child, depth + 1);
        }
    }
}

#[test]
fn rust_reports_each_kind_of_change() {
    let before = "\
use std::fmt;

/// Builds things.
pub struct Builder {
    size: usize,
}

impl Builder {
    pub fn new() -> Self {
        Builder { size: 0 }
    }

    /// Grows by one.
    pub fn grow(&mut self) {
        self.size += 1;
    }

    pub fn shrink(&mut self) {
        // smaller
        self.size -= 1;
    }

    fn old_name(&self) -> usize {
        let doubled = self.size * 2;
        let tripled = self.size * 3;
        doubled + tripled
    }
}

fn gone() {}

const LIMIT: usize = 10;
";
    let after = "\
use std::fmt;
use std::io;

/// Builds things.
pub struct Builder {
    size: usize,
}

impl Builder {
    pub fn new() -> Self {
        Builder { size: 0 }
    }

    /// Grows by one, or more.
    pub fn grow(&mut self, by: usize) {
        self.size += by;
    }

    pub fn shrink(&mut self) {
        // smaller still
        self.size -= 1;
    }

    fn new_name(&self) -> usize {
        let doubled = self.size * 2;
        let tripled = self.size * 3;
        doubled + tripled
    }

    pub fn reset(&mut self) {
        self.size = 0;
    }
}

const LIMIT: usize =   10;
";
    assert_eq!(
        report("a.rs", before, after),
        "in  impl  Builder  old 8-28  new 9-33  +1 -0\n  modified  method  grow  [signature, body, docs]  old 13-16  new 14-17  +3 -3\n    sig- pub fn grow(&mut self)\n    sig+ pub fn grow(&mut self, by: usize)\n  modified  method  shrink  [comments]  old 18-21  new 19-22  +1 -1\n  renamed from old_name:  method  new_name  old 23-27  new 24-28  +1 -1\n  added  method  reset  new 30-32  +3 -0\n    sig+ pub fn reset(&mut self)\nremoved  function  gone  old 30-30  +0 -1\n  sig- fn gone()\nmodified  constant  LIMIT  [formatting]  old 32-32  new 35-35  +1 -1\ntop-level  [imports]  at 2\n"
    );
}

/// One file per language: `(path, before, after, expected report)`.
const LANGUAGE_CASES: &[(&str, &str, &str, &str)] = &[
    (
        "a.py",
        "import os\n\nLIMIT = 1\n\nclass Shop:\n    def total(self, items):\n        return sum(items)\n\n    def tax(self):\n        # flat\n        return 2\n\n    def old(self):\n        pass\n\ndef gone():\n    pass\n",
        "import os\nimport sys\n\nLIMIT = 2\n\nclass Shop:\n    def total(self, items, discount):\n        return sum(items) - discount\n\n    def tax(self):\n        # flat rate\n        return 2\n\n    def fresh(self):\n        pass\n",
        "modified  variable  LIMIT  [body]  old 3-3  new 4-4  +1 -1\nin  class  Shop  old 5-14  new 6-15\n  modified  method  total  [signature, body]  old 6-7  new 7-8  +2 -2\n    sig- def total(self, items):\n    sig+ def total(self, items, discount):\n  modified  method  tax  [comments]  old 9-11  new 10-12  +1 -1\n  renamed from old:  method  fresh  old 13-14  new 14-15  +1 -1\nremoved  function  gone  old 16-17  +0 -2\n  sig- def gone():\ntop-level  [imports]  at 2\n",
    ),
    (
        "a.js",
        "import x from 'y';\nexport const LIMIT = 1;\nexport function total(items) {\n  return items.length;\n}\nfunction tax() {\n  // flat\n  return 2;\n}\nfunction gone() {}\ndescribe(\"cart\", () => {\n  it(\"sums\", () => {\n    expect(1).toBe(1);\n  });\n});\n",
        "import x from 'y';\nexport const LIMIT = 2;\nexport function total(items, discount) {\n  return items.length - discount;\n}\nfunction tax() {\n  // flat rate\n  return 2;\n}\ndescribe(\"cart\", () => {\n  it(\"sums\", () => {\n    expect(2).toBe(2);\n  });\n  it(\"is empty\", () => {});\n});\n",
        "modified  variable  LIMIT  [body]  old 2-2  new 2-2  +1 -1\nmodified  function  total  [signature, body]  old 3-5  new 3-5  +2 -2\n  sig- export function total(items)\n  sig+ export function total(items, discount)\nmodified  function  tax  [comments]  old 6-9  new 6-9  +1 -1\nremoved  function  gone  old 10-10  +0 -1\n  sig- function gone()\nin  test  describe(\"cart\")  old 11-15  new 10-15\n  modified  test  it(\"sums\")  [body]  old 12-14  new 11-13  +1 -1\n  added  test  it(\"is empty\")  new 14-14  +1 -0\n    sig+ it(\"is empty\", () =>\n",
    ),
    (
        "a.ts",
        "interface Item {\n  price: number;\n}\nexport type Id = string;\nexport class Cart {\n  total(items: Item[]): number {\n    return 0;\n  }\n}\nfunction o(a: string): void;\nfunction o(a: any) {}\n",
        "interface Item {\n  price: number;\n  name: string;\n}\nexport type Id = number;\nexport class Cart {\n  total(items: Item[], discount: number): number {\n    return 0;\n  }\n}\nfunction o(a: string): void;\nfunction o(a: number): void;\nfunction o(a: any) {}\n",
        "modified  interface  Item  [body]  old 1-3  new 1-4  +1 -0\nmodified  type alias  Id  [body]  old 4-4  new 5-5  +1 -1\nin  class  Cart  old 5-9  new 6-10\n  modified  method  total  [signature]  old 6-8  new 7-9  +1 -1\n    sig- total(items: Item[]): number\n    sig+ total(items: Item[], discount: number): number\nadded  function  o  new 12-12  +1 -0\n  sig+ function o(a: number): void;\n",
    ),
    (
        "a.go",
        "package p\n\nconst Limit = 1\n\ntype Server struct {\n\tport int\n}\n\nfunc (s *Server) Start() {\n\ts.port = 1\n}\n\nfunc (s *Server) Stop() {\n}\n\nfunc gone() {}\n",
        "package p\n\nconst Limit = 2\n\ntype Server struct {\n\tport int\n\thost string\n}\n\nfunc (s *Server) Start(port int) {\n\ts.port = port\n}\n\nfunc (s *Server) Stop() {\n}\n",
        "modified  constant  Limit  [body]  old 3-3  new 3-3  +1 -1\nmodified  type  Server  [body]  old 5-7  new 5-8  +1 -0\nmodified  method  Server.Start  [signature, body]  old 9-11  new 10-12  +2 -2\n  sig- func (s *Server) Start()\n  sig+ func (s *Server) Start(port int)\nremoved  function  gone  old 16-16  +0 -1\n  sig- func gone()\n",
    ),
    (
        "a.rb",
        "LIMIT = 1\n\nclass Shop\n  def total(items)\n    items.sum\n  end\n\n  def self.build\n    new\n  end\n\n  def gone\n  end\nend\n",
        "LIMIT = 2\n\nclass Shop\n  def total(items, discount)\n    items.sum - discount\n  end\n\n  def self.build\n    # cached\n    new\n  end\nend\n",
        "modified  constant  LIMIT  [body]  old 1-1  new 1-1  +1 -1\nin  class  Shop  old 3-14  new 3-12  +0 -1\n  modified  method  total  [signature, body]  old 4-6  new 4-6  +2 -2\n    sig- def total(items)\n    sig+ def total(items, discount)\n  modified  method  self.build  [comments]  old 8-10  new 8-11  +1 -0\n  removed  method  gone  old 12-13  +0 -2\n    sig- def gone\n",
    ),
    (
        "a.kt",
        "val limit = 1\n\nclass Shop {\n    fun total(items: List<Int>): Int {\n        return items.sum()\n    }\n\n    fun gone() {}\n}\n",
        "val limit = 2\n\nclass Shop {\n    fun total(items: List<Int>, discount: Int): Int {\n        return items.sum() - discount\n    }\n}\n",
        "modified  variable  limit  [body]  old 1-1  new 1-1  +1 -1\nin  class  Shop  old 3-9  new 3-7  +0 -1\n  modified  method  total  [signature, body]  old 4-6  new 4-6  +2 -2\n    sig- fun total(items: List<Int>): Int\n    sig+ fun total(items: List<Int>, discount: Int): Int\n  removed  method  gone  old 8-8  +0 -1\n    sig- fun gone()\n",
    ),
    (
        "A.java",
        "class Shop {\n  int limit = 1;\n\n  int total(int a) {\n    return a;\n  }\n\n  void gone() {}\n}\n",
        "class Shop {\n  int limit = 2;\n\n  int total(int a, int b) {\n    return a + b;\n  }\n}\n",
        "in  class  Shop  old 1-9  new 1-7  +0 -1\n  modified  variable  limit  [body]  old 2-2  new 2-2  +1 -1\n  modified  method  total  [signature, body]  old 4-6  new 4-6  +2 -2\n    sig- int total(int a)\n    sig+ int total(int a, int b)\n  removed  method  gone  old 8-8  +0 -1\n    sig- void gone()\n",
    ),
    (
        "a.c",
        "#include <stdio.h>\n#define LIMIT 1\nint total(int a);\nint total(int a) {\n    return a;\n}\nstatic int gone(void) { return 0; }\n",
        "#include <stdio.h>\n#include <stdlib.h>\n#define LIMIT 2\nint total(int a, int b);\nint total(int a, int b) {\n    return a + b;\n}\n",
        "modified  macro  LIMIT  [body]  old 2-2  new 3-3  +1 -1\nmodified  function  total  [signature]  old 3-3  new 4-4  +1 -1\n  sig- int total(int a);\n  sig+ int total(int a, int b);\nmodified  function  total  [signature, body]  old 4-6  new 5-7  +2 -2\n  sig- int total(int a)\n  sig+ int total(int a, int b)\nremoved  function  gone  old 7-7  +0 -1\n  sig- static int gone(void)\ntop-level  [imports]  at 2\n",
    ),
    (
        "a.cpp",
        "class Shop {\n public:\n  int total(int a);\n};\nint Shop::total(int a) {\n  return a;\n}\n",
        "class Shop {\n public:\n  int total(int a, int b);\n};\nint Shop::total(int a, int b) {\n  return a + b;\n}\n",
        "in  class  Shop  old 1-4  new 1-4\n  modified  method  total  [signature]  old 3-3  new 3-3  +1 -1\n    sig- int total(int a);\n    sig+ int total(int a, int b);\nmodified  function  Shop::total  [signature, body]  old 5-7  new 5-7  +2 -2\n  sig- int Shop::total(int a)\n  sig+ int Shop::total(int a, int b)\n",
    ),
    (
        "a.sh",
        "build() {\n  make\n}\n\ngone() {\n  true\n}\n",
        "build() {\n  make all\n}\n",
        "modified  function  build  [body]  old 1-3  new 1-3  +1 -2\nremoved  function  gone  old 5-7  +0 -2\n  sig- gone()\n",
    ),
    (
        "a.lua",
        "local M = {}\nfunction M.total(items)\n  return #items\nend\nM.tax = function() return 2 end\nreturn M\n",
        "local M = {}\nfunction M.total(items, discount)\n  return #items - discount\nend\nM.tax = function() return 3 end\nreturn M\n",
        "modified  function  M.total  [signature, body]  old 2-4  new 2-4  +2 -2\n  sig- function M.total(items)\n  sig+ function M.total(items, discount)\nmodified  function  M.tax  [body]  old 5-5  new 5-5  +1 -1\n",
    ),
    (
        "a.css",
        ".a { color: red; }\n@media (max-width: 600px) {\n  .b { color: blue; }\n}\n.gone { x: 1 }\n",
        ".a { color: green; }\n@media (max-width: 600px) {\n  .b { color: blue; }\n  .c { color: blue; }\n}\n",
        "modified  rule  .a  [body]  old 1-1  new 1-1  +1 -1\nin  block  @media (max-width: 600px)  old 2-4  new 2-5\n  added  rule  .c  new 4-4  +1 -0\n    sig+ .c\nremoved  rule  .gone  old 5-5  +0 -1\n  sig- .gone\n",
    ),
    (
        "a.tf",
        "region = \"x\"\nresource \"aws_s3_bucket\" \"logs\" {\n  bucket = \"x\"\n}\n",
        "region = \"y\"\nresource \"aws_s3_bucket\" \"logs\" {\n  bucket = \"x\"\n  acl = \"private\"\n}\n",
        "modified  variable  region  [body]  old 1-1  new 1-1  +1 -1\nin  block  resource \"aws_s3_bucket\" \"logs\"  old 2-4  new 2-5\n  added  variable  acl  new 4-4  +1 -0\n    sig+ acl\n",
    ),
    (
        "a.md",
        "# Title\n\nText.\n\n## Install\n\nRun it.\n\n## Old\n\nGone.\n",
        "# Title\n\nText.\n\n## Install\n\nRun it now.\n\n## New\n\nHere.\n",
        "in  section  Title  old 1-11  new 1-11\n  modified  section  Install  [body]  old 5-8  new 5-8  +1 -1\n  removed  section  Old  old 9-11  +0 -2\n    sig- ## Old\n  added  section  New  new 9-11  +2 -0\n    sig+ ## New\n",
    ),
    (
        "a.sql",
        "CREATE TABLE users (\n  id int\n);\nCREATE TABLE gone (\n  id int\n);\n",
        "CREATE TABLE users (\n  id int,\n  name text\n);\n",
        "modified  statement  users  [body]  old 1-3  new 1-4  +2 -2\nremoved  statement  gone  old 4-6  +0 -2\n  sig- CREATE TABLE gone\n",
    ),
    (
        "Cargo.toml",
        "[package]\nname = \"x\"\nversion = \"1\"\n\n[dependencies]\nserde = \"1\"\nold = \"1\"\n",
        "[package]\nname = \"x\"\nversion = \"2\"\n\n[dependencies]\nserde = \"1\"\nnew = \"1\"\n",
        "in  table  package  old 1-4  new 1-4\n  modified  key  version  [body]  old 3-3  new 3-3  +1 -1\nin  table  dependencies  old 5-7  new 5-7\n  removed  key  old  old 7-7  +0 -1\n    sig- old\n  added  key  new  new 7-7  +1 -0\n    sig+ new\n",
    ),
    (
        "a.json",
        "{\n  \"name\": \"x\",\n  \"scripts\": {\n    \"build\": \"tsc\",\n    \"test\": \"jest\"\n  }\n}\n",
        "{\n  \"name\": \"y\",\n  \"scripts\": {\n    \"build\": \"tsc -b\",\n    \"lint\": \"eslint\"\n  }\n}\n",
        "modified  key  name  [body]  old 2-2  new 2-2  +1 -1\nin  key  scripts  old 3-6  new 3-6\n  modified  key  build  [body]  old 4-4  new 4-4  +1 -1\n  removed  key  test  old 5-5  +0 -1\n    sig- \"test\":\n  added  key  lint  new 5-5  +1 -0\n    sig+ \"lint\":\n",
    ),
    (
        "a.yaml",
        "name: x\njobs:\n  build:\n    runs-on: ubuntu\n  old:\n    runs-on: mac\n",
        "name: y\njobs:\n  build:\n    runs-on: ubuntu-latest\n",
        "modified  key  name  [body]  old 1-1  new 1-1  +1 -1\nin  key  jobs  old 2-6  new 2-4\n  modified  key  build  [body]  old 3-4  new 3-4  +1 -1\n  removed  key  old  old 5-6  +0 -2\n    sig- old:\n",
    ),
];

#[test]
fn each_language_reports_signature_body_comment_and_membership_changes() {
    let mut failures = Vec::new();
    for (path, before, after, expected) in LANGUAGE_CASES {
        let actual = report(path, before, after);
        if actual != *expected {
            failures.push(format!(
                "{path}\n--- expected\n{expected}--- actual\n{actual}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_method_moved_between_impls_is_moved_from_its_old_parent() {
    assert_eq!(
        report(
            "a.rs",
            "struct A;\nstruct B;\n\nimpl A {\n    fn keep(&self) {}\n\n    fn travel(&self) -> u8 {\n        1\n    }\n}\n\nimpl B {\n    fn other(&self) {}\n}\n",
            "struct A;\nstruct B;\n\nimpl A {\n    fn keep(&self) {}\n}\n\nimpl B {\n    fn other(&self) {}\n\n    fn travel(&self) -> u8 {\n        1\n    }\n}\n"
        ),
        "in  impl  B  old 12-14  new 8-14  +1 -0\n  moved from A:  method  travel  old 7-9  new 11-13  +3 -3\n"
    );
}

#[test]
fn a_function_moved_within_the_file_is_moved() {
    assert_eq!(
        report(
            "a.rs",
            "fn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n\nfn c() {\n    three();\n}\n",
            "fn b() {\n    two();\n}\n\nfn a() {\n    one();\n}\n\nfn c() {\n    three();\n}\n"
        ),
        "moved:  function  a  old 1-3  new 5-7  +3 -3\n"
    );
}

#[test]
fn code_shifted_by_an_insertion_above_it_is_not_reported() {
    assert_eq!(
        report(
            "a.rs",
            "fn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n",
            "fn new() {\n    zero();\n}\n\nfn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n"
        ),
        "added  function  new  new 1-3  +3 -0\n  sig+ fn new()\n"
    );
}

#[test]
fn two_impls_of_one_type_pair_in_order() {
    assert_eq!(
        report(
            "a.rs",
            "impl Foo {\n    fn a(&self) {}\n}\n\nimpl Foo {\n    fn b(&self) {}\n}\n",
            "impl Foo {\n    fn a(&self) {}\n}\n\nimpl Foo {\n    fn b(&self) {\n        work();\n    }\n}\n"
        ),
        "in  impl  Foo  old 5-7  new 5-9\n  modified  method  b  [body]  old 6-6  new 6-8  +3 -1\n"
    );
}

#[test]
fn reindenting_is_formatting_only() {
    assert_eq!(
        report(
            "a.rs",
            "fn a() {\n    let x = 1;\n    x + 1\n}\n",
            "fn a() {\n  let x = 1;\n  x + 1\n}\n"
        ),
        "modified  function  a  [formatting]  old 1-4  new 1-4  +2 -2\n"
    );
}

#[test]
fn a_syntax_error_marks_the_file_and_the_definition() {
    assert_eq!(
        report(
            "a.rs",
            "fn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n",
            "fn a() {\n    one(;\n}\n\nfn b() {\n    two();\n}\n"
        ),
        "syntax errors\nmodified  function  a  [body]  old 1-3  new 1-3  +1 -1  uncertain\n"
    );
}

#[test]
fn a_new_file_adds_everything_it_defines() {
    assert_eq!(
        report(
            "a.rs",
            "",
            "use std::fmt;\n\nfn a() {}\n\nimpl S {\n    fn b(&self) {}\n}\n"
        ),
        "added  function  a  new 3-3  +1 -0\n  sig+ fn a()\nadded  impl  S  new 5-7  +2 -0\n  sig+ impl S\ntop-level  [imports]  at 1\n"
    );
}

#[test]
fn a_deleted_file_removes_everything_it_defined() {
    assert_eq!(
        report("a.rs", "fn a() {}\n\nstruct S;\n", ""),
        "removed  function  a  old 1-1  +0 -1\n  sig- fn a()\nremoved  struct  S  old 3-3  +0 -1\n  sig- struct S;\n"
    );
}

#[test]
fn an_unchanged_file_reports_nothing() {
    assert_eq!(
        report(
            "a.rs",
            "fn a() {\n    one();\n}\n",
            "fn a() {\n    one();\n}\n"
        ),
        ""
    );
}

#[test]
fn a_renamed_function_with_an_edited_body_is_renamed() {
    assert_eq!(
        report(
            "a.rs",
            "fn compute_total(items: &[u32]) -> u32 {\n    let mut total = 0;\n    for item in items {\n        total += item;\n    }\n    total\n}\n",
            "fn sum_items(items: &[u32]) -> u32 {\n    let mut total = 0;\n    for item in items {\n        total += *item;\n    }\n    total\n}\n"
        ),
        "renamed from compute_total:  function  sum_items  [body]  old 1-7  new 1-7  +2 -2\n"
    );
}

#[test]
fn a_doc_comment_edit_is_docs_only() {
    assert_eq!(
        report("a.rs", "/// Old.\nfn a() {}\n", "/// New.\nfn a() {}\n"),
        "modified  function  a  [docs]  old 1-2  new 1-2  +1 -1\n"
    );
}

#[test]
fn module_docs_above_a_function_are_top_level_comments() {
    assert_eq!(
        report(
            "a.rs",
            "//! Old.\n\nfn first() {\n    1;\n}\n",
            "//! New.\n\nfn first() {\n    2;\n}\n"
        ),
        "modified  function  first  [body]  old 3-5  new 3-5  +1 -1\ntop-level  [comments]  at 1\n"
    );
}

#[test]
fn an_attribute_edit_is_attributes_only() {
    assert_eq!(
        report(
            "a.rs",
            "#[inline]\nfn a() {}\n",
            "#[inline(always)]\nfn a() {}\n"
        ),
        "modified  function  a  [attributes]  old 1-2  new 1-2  +1 -1\n"
    );
}

#[test]
fn a_diff_keeps_its_definition_changes_across_expansion() {
    let before = "fn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n";
    let after = "fn a() {\n    one();\n}\n\nfn b() {\n    three();\n}\n";
    let diff = deltoids::Diff::compute(before, after, "a.rs");
    let expected = DefinitionDiff::compute(before, after, "a.rs");
    assert_eq!(diff.definitions(), expected.as_ref());
    let expanded = diff.expand(0).expect("expands");
    assert_eq!(expanded.definitions(), expected.as_ref());
    assert!(
        deltoids::Diff::compute("x", "y", "notes.txt")
            .definitions()
            .is_none()
    );
}

/// Every definition in `definitions`, one per line as `status name`,
/// nested ones indented under their parent.
fn tree(definitions: &[DefinitionChange], depth: usize, out: &mut Vec<String>) {
    for definition in definitions {
        let status = match definition.status {
            Status::Modified => "modified",
            Status::Container => "in",
            Status::Unchanged => "unchanged",
            Status::Added => "added",
            Status::Removed => "removed",
            Status::Renamed { .. } => "renamed",
            Status::Moved { .. } => "moved",
        };
        out.push(format!(
            "{}{status} {}",
            "  ".repeat(depth),
            definition.name
        ));
        tree(&definition.children, depth + 1, out);
    }
}

#[test]
fn unchanged_definitions_stand_around_the_changes_as_context() {
    let before = "\
fn first() {
    1;
}

impl Cart {
    fn total(&self) -> u32 {
        1
    }

    fn count(&self) -> usize {
        2
    }
}

impl Untouched {
    fn get(&self) -> u8 {
        3
    }
}
";
    let after = before.replace("        1\n", "        10\n");
    let diff = DefinitionDiff::compute(before, &after, "a.rs").expect("rust");
    let mut lines = Vec::new();
    tree(&diff.definitions, 0, &mut lines);
    assert_eq!(
        lines,
        [
            "unchanged first",
            "in Cart",
            "  modified total",
            "  unchanged count",
            "unchanged Untouched",
        ]
    );
    assert!(!diff.is_empty());
    let same = DefinitionDiff::compute(before, before, "a.rs").expect("rust");
    assert!(same.is_empty());
    assert!(
        same.definitions
            .iter()
            .all(|definition| !definition.is_changed())
    );
}

#[test]
fn each_stretch_of_code_outside_definitions_changes_where_it_is() {
    let before = "\
//! Old docs.

use std::fmt;

fn first() {}

fn last() {}

// The end.
";
    let after = before
        .replace("//! Old docs.", "//! New docs.\n//! More.")
        .replace("// The end.", "// The very end.")
        .replace("use std::fmt;", "use std::fmt;\nuse std::io;");
    let diff = DefinitionDiff::compute(before, &after, "a.rs").expect("rust");
    let tops: Vec<(String, usize)> = diff
        .top_level
        .iter()
        .map(|top| (aspects(&top.aspects), top.at))
        .collect();
    let line = |text: &str| after.lines().position(|line| line == text).unwrap() + 1;
    assert_eq!(
        tops,
        [
            // No definition between the docs and the imports: one change.
            ("[comments, imports]".to_string(), 1),
            ("[comments]".to_string(), line("// The very end.")),
        ]
    );
}
