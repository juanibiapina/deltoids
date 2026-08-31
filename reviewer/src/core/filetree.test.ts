import { describe, expect, test } from "vitest";
import { buildTree, directoryIds, ROOT_ID, type TreeNode } from "./filetree";

function files(...specs: [string, string?][]): { filename: string; status: string }[] {
  return specs.map(([filename, status]) => ({ filename, status: status ?? "modified" }));
}

// Convenience: the pre-order list of visible nodes (root excluded), each as
// [name, isDir]. Order follows the emit walk, which is deterministic.
function outline(nodes: TreeNode[]): [string, boolean][] {
  return nodes
    .filter((n) => n.id !== ROOT_ID)
    .map((n) => [n.name, n.metadata.isDir] as [string, boolean]);
}

function byId(nodes: TreeNode[], id: string): TreeNode {
  const n = nodes.find((x) => x.id === id);
  if (!n) throw new Error(`no node ${id}`);
  return n;
}

// Mirrors crates/deltoids-cli/src/sidebar/tree.rs::build_rows tests.

test("groups files under a common directory", () => {
  const nodes = buildTree(files(["src/a.rs"], ["src/b.rs"]));
  expect(outline(nodes)).toEqual([
    ["src", true],
    ["a.rs", false],
    ["b.rs", false],
  ]);
  const src = byId(nodes, "src");
  expect(src.children).toEqual(["src/a.rs", "src/b.rs"]);
});

test("collapses a single-child directory chain", () => {
  const nodes = buildTree(
    files(["crates/deltoids/src/lib.rs"], ["crates/deltoids/src/parse.rs"]),
  );
  expect(outline(nodes)).toEqual([
    ["crates/deltoids/src", true],
    ["lib.rs", false],
    ["parse.rs", false],
  ]);
  // The folded directory keeps the full path as its id.
  expect(byId(nodes, "crates/deltoids/src").metadata.isDir).toBe(true);
});

test("does not collapse when a directory has multiple children", () => {
  const nodes = buildTree(
    files(["crates/deltoids/src/lib.rs"], ["crates/deltoids-cli/src/lib.rs"]),
  );
  expect(outline(nodes)).toEqual([
    ["crates", true],
    ["deltoids/src", true],
    ["lib.rs", false],
    ["deltoids-cli/src", true],
    ["lib.rs", false],
  ]);
});

test("handles top-level files", () => {
  const nodes = buildTree(files(["README.md"], ["Cargo.toml"]));
  // Sorted: Cargo.toml before README.md (ordinal).
  expect(outline(nodes)).toEqual([
    ["Cargo.toml", false],
    ["README.md", false],
  ]);
  expect(byId(nodes, ROOT_ID).children).toEqual(["Cargo.toml", "README.md"]);
});

test("sorts dirs and files interleaved by name", () => {
  const nodes = buildTree(files(["zzz.rs"], ["src/a.rs"], ["aaa.rs"]));
  // Expect (mixed): aaa.rs ; src/ ; src/a.rs ; zzz.rs
  expect(outline(nodes)).toEqual([
    ["aaa.rs", false],
    ["src", true],
    ["a.rs", false],
    ["zzz.rs", false],
  ]);
});

describe("leaf metadata", () => {
  test("carries fileIndex, status and full path", () => {
    const nodes = buildTree([
      { filename: "src/a.rs", status: "added" },
      { filename: "old.rs", status: "removed" },
    ]);
    const a = byId(nodes, "src/a.rs");
    expect(a.metadata).toEqual({
      isDir: false,
      fileIndex: 0,
      status: "added",
      path: "src/a.rs",
    });
    const old = byId(nodes, "old.rs");
    expect(old.metadata.fileIndex).toBe(1);
    expect(old.metadata.status).toBe("removed");
  });
});

test("directoryIds lists branches, excluding the root", () => {
  const nodes = buildTree(files(["src/a.rs"], ["crates/x/y/z.rs"]));
  expect(directoryIds(nodes).sort()).toEqual(["crates/x/y", "src"]);
});
