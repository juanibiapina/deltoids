//! Symbol-extraction tests, per language.

use super::*;

fn names_kinds(symbols: &[Symbol]) -> Vec<(SymbolKind, String)> {
    symbols
        .iter()
        .map(|s| (s.kind.clone(), s.qualified_name()))
        .collect()
}

// ---------------------------------------------------------------------------
// Rust
// ---------------------------------------------------------------------------

#[test]
fn rust_extracts_top_level_function() {
    let src = "fn hello() {}\n";
    let symbols = extract_symbols("a.rs", src);
    assert_eq!(
        names_kinds(&symbols),
        vec![(SymbolKind::Function, "hello".to_string())]
    );
    assert_eq!(symbols[0].visibility, Visibility::Private);
}

#[test]
fn rust_marks_pub_function_public() {
    let src = "pub fn hello() {}\n";
    let symbols = extract_symbols("a.rs", src);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].visibility, Visibility::Public);
}

#[test]
fn rust_marks_pub_crate_function_crate() {
    let src = "pub(crate) fn hello() {}\n";
    let symbols = extract_symbols("a.rs", src);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].visibility, Visibility::Crate);
}

#[test]
fn rust_extracts_struct_and_impl_methods() {
    let src = "\
pub struct Foo;

impl Foo {
    pub fn compute(&self) -> i32 { 42 }
    fn helper(&self) -> i32 { 1 }
}
";
    let symbols = extract_symbols("a.rs", src);
    let nk = names_kinds(&symbols);
    assert!(
        nk.contains(&(SymbolKind::Struct, "Foo".to_string())),
        "missing Foo: {nk:?}"
    );
    assert!(
        nk.contains(&(SymbolKind::Method, "Foo::compute".to_string())),
        "missing Foo::compute: {nk:?}"
    );
    assert!(
        nk.contains(&(SymbolKind::Method, "Foo::helper".to_string())),
        "missing Foo::helper: {nk:?}"
    );
}

#[test]
fn rust_extracts_trait_and_methods() {
    let src = "\
pub trait Drawable {
    fn draw(&self);
}
";
    let symbols = extract_symbols("a.rs", src);
    let nk = names_kinds(&symbols);
    assert!(nk.contains(&(SymbolKind::Trait, "Drawable".to_string())));
    assert!(nk.contains(&(SymbolKind::Method, "Drawable::draw".to_string())));
}

#[test]
fn rust_signature_excludes_body() {
    let src = "pub fn hello(x: i32) -> i32 {\n    x + 1\n}\n";
    let symbols = extract_symbols("a.rs", src);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].signature, "pub fn hello(x: i32) -> i32");
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

#[test]
fn python_extracts_module_function() {
    let src = "def hello():\n    pass\n";
    let symbols = extract_symbols("a.py", src);
    assert_eq!(
        names_kinds(&symbols),
        vec![(SymbolKind::Function, "hello".to_string())]
    );
}

#[test]
fn python_class_methods_become_methods() {
    let src = "\
class Foo:
    def bar(self):
        pass
    def _private(self):
        pass
";
    let symbols = extract_symbols("a.py", src);
    let nk = names_kinds(&symbols);
    assert!(nk.contains(&(SymbolKind::Class, "Foo".to_string())));
    assert!(nk.contains(&(SymbolKind::Method, "Foo::bar".to_string())));
    assert!(nk.contains(&(SymbolKind::Method, "Foo::_private".to_string())));

    let priv_sym = symbols
        .iter()
        .find(|s| s.qualified_name() == "Foo::_private")
        .unwrap();
    assert_eq!(priv_sym.visibility, Visibility::Private);
    let pub_sym = symbols
        .iter()
        .find(|s| s.qualified_name() == "Foo::bar")
        .unwrap();
    assert_eq!(pub_sym.visibility, Visibility::Public);
}

// ---------------------------------------------------------------------------
// TypeScript
// ---------------------------------------------------------------------------

#[test]
fn typescript_export_is_public() {
    let src = "export function hello(): void {}\nfunction internal(): void {}\n";
    let symbols = extract_symbols("a.ts", src);
    assert_eq!(symbols.len(), 2);

    let hello = symbols.iter().find(|s| s.path == ["hello"]).unwrap();
    assert_eq!(hello.visibility, Visibility::Public);

    let internal = symbols.iter().find(|s| s.path == ["internal"]).unwrap();
    assert_eq!(internal.visibility, Visibility::Private);
}

#[test]
fn typescript_class_methods() {
    let src = "\
export class Foo {
  bar(): number { return 1; }
  private baz(): number { return 2; }
}
";
    let symbols = extract_symbols("a.ts", src);
    let nk = names_kinds(&symbols);
    assert!(nk.contains(&(SymbolKind::Class, "Foo".to_string())));
    assert!(nk.contains(&(SymbolKind::Method, "Foo::bar".to_string())));
    assert!(nk.contains(&(SymbolKind::Method, "Foo::baz".to_string())));
}

#[test]
fn typescript_interface_extracted_as_trait() {
    let src = "export interface Drawable { draw(): void; }\n";
    let symbols = extract_symbols("a.ts", src);
    let nk = names_kinds(&symbols);
    assert!(nk.contains(&(SymbolKind::Trait, "Drawable".to_string())));
}

// ---------------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------------

#[test]
fn go_extracts_function() {
    let src = "package main\n\nfunc Hello() {}\nfunc helper() {}\n";
    let symbols = extract_symbols("a.go", src);
    let nk = names_kinds(&symbols);
    assert!(nk.contains(&(SymbolKind::Function, "Hello".to_string())));
    assert!(nk.contains(&(SymbolKind::Function, "helper".to_string())));

    let hello = symbols.iter().find(|s| s.path == ["Hello"]).unwrap();
    assert_eq!(hello.visibility, Visibility::Public);
    let helper = symbols.iter().find(|s| s.path == ["helper"]).unwrap();
    assert_eq!(helper.visibility, Visibility::Private);
}

// ---------------------------------------------------------------------------
// Java
// ---------------------------------------------------------------------------

#[test]
fn java_class_methods_visibility() {
    let src = "\
public class Foo {
  public int bar() { return 1; }
  private int baz() { return 2; }
}
";
    let symbols = extract_symbols("a.java", src);
    let nk = names_kinds(&symbols);
    assert!(nk.contains(&(SymbolKind::Class, "Foo".to_string())));
    assert!(nk.contains(&(SymbolKind::Method, "Foo::bar".to_string())));
    assert!(nk.contains(&(SymbolKind::Method, "Foo::baz".to_string())));

    let bar = symbols
        .iter()
        .find(|s| s.qualified_name() == "Foo::bar")
        .unwrap();
    assert_eq!(bar.visibility, Visibility::Public);
    let baz = symbols
        .iter()
        .find(|s| s.qualified_name() == "Foo::baz")
        .unwrap();
    assert_eq!(baz.visibility, Visibility::Private);
}

// ---------------------------------------------------------------------------
// Spans
// ---------------------------------------------------------------------------

#[test]
fn span_covers_full_declaration_including_body() {
    let src = "\
pub fn hello() {
    println!(\"hi\");
}
";
    let symbols = extract_symbols("a.rs", src);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].span, LineSpan::new(1, 3));
    assert_eq!(symbols[0].body_span, Some(LineSpan::new(2, 2)));
}

// ---------------------------------------------------------------------------
// Robustness
// ---------------------------------------------------------------------------

#[test]
fn unsupported_language_returns_empty() {
    assert!(extract_symbols("a.unknown", "anything").is_empty());
}

#[test]
fn empty_source_returns_empty() {
    assert!(extract_symbols("a.rs", "").is_empty());
}
