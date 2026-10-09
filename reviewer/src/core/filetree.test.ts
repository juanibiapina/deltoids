import { describe, expect, test } from "vitest";
import {
  buildTree,
  directoryIds,
  displayOrder,
  pruneReviewed,
  selectionFiles,
  stepSelection,
  ROOT_ID,
  type TreeNode,
} from "./filetree";

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

describe("pruneReviewed", () => {
  test("returns the same array when nothing is reviewed", () => {
    const nodes = buildTree(files(["src/a.rs"], ["src/b.rs"]));
    expect(pruneReviewed(nodes, () => false)).toBe(nodes);
  });

  test("drops a reviewed leaf but keeps the directory with siblings", () => {
    const nodes = buildTree(files(["src/a.rs"], ["src/b.rs"]));
    // a.rs is fileIndex 0.
    const pruned = pruneReviewed(nodes, (i) => i === 0);
    expect(outline(pruned)).toEqual([
      ["src", true],
      ["b.rs", false],
    ]);
    // The surviving directory no longer references the dropped child.
    expect(byId(pruned, "src").children).toEqual(["src/b.rs"]);
  });

  test("removes a directory that becomes empty, cascading upward", () => {
    const nodes = buildTree(files(["crates/x/y/z.rs"], ["src/a.rs"]));
    // z.rs is fileIndex 0; pruning it should drop the whole crates/x/y chain.
    const pruned = pruneReviewed(nodes, (i) => i === 0);
    expect(outline(pruned)).toEqual([
      ["src", true],
      ["a.rs", false],
    ]);
  });
});

describe("selection", () => {
  const nodes = buildTree(
    files(["z.md"], ["ab/x.ts"], ["a/b/c.ts"], ["a/b/d/e.ts"], ["a/f.ts"]),
  );

  test("display order follows the tree, not the input", () => {
    expect(displayOrder(nodes)).toEqual([2, 3, 4, 1, 0]);
  });

  test("a file selection shows just that file", () => {
    expect(selectionFiles(nodes, { kind: "file", index: 4 })).toEqual([4]);
  });

  test("a directory shows its subtree in display order", () => {
    expect(selectionFiles(nodes, { kind: "dir", id: "a" })).toEqual([2, 3, 4]);
    expect(selectionFiles(nodes, { kind: "dir", id: "a/b/d" })).toEqual([3]);
  });

  test("a sibling sharing a name prefix stays out", () => {
    expect(selectionFiles(nodes, { kind: "dir", id: "ab" })).toEqual([1]);
  });

  test("a collapsed directory chain selects by its joined id", () => {
    const chain = buildTree(files(["x/y/z/1.ts"], ["x/y/z/2.ts"], ["w.ts"]));
    expect(directoryIds(chain)).toEqual(["x/y/z"]);
    expect(selectionFiles(chain, { kind: "dir", id: "x/y/z" })).toEqual([0, 1]);
  });
});

describe("stepSelection", () => {
  // Rows: a/ (dir) · a/b/ (dir) · c.ts (2) · d/ (dir) · e.ts (3) · f.ts (4) · ab/ (dir) · x.ts (1) · z.md (0)
  const nodes = buildTree(
    files(["z.md"], ["ab/x.ts"], ["a/b/c.ts"], ["a/b/d/e.ts"], ["a/f.ts"]),
  );
  const open = { collapsed: new Set<string>() };
  const walk = (start: Parameters<typeof stepSelection>[1], direction: 1 | -1, opts = open) => {
    const out = [];
    let sel = start;
    for (;;) {
      const next = stepSelection(nodes, sel, direction, opts);
      if (next === sel) return out;
      out.push(next.kind === "file" ? next.index : next.id);
      sel = next;
    }
  };

  test("steps through directories and files in tree order", () => {
    expect(walk({ kind: "dir", id: "a" }, 1)).toEqual(["a/b", 2, "a/b/d", 3, 4, "ab", 1, 0]);
    expect(walk({ kind: "file", index: 0 }, -1)).toEqual([1, "ab", 4, 3, "a/b/d", 2, "a/b", "a"]);
  });

  test("returns the same selection at either end", () => {
    const first = { kind: "dir", id: "a" } as const;
    const last = { kind: "file", index: 0 } as const;
    expect(stepSelection(nodes, first, -1, open)).toBe(first);
    expect(stepSelection(nodes, last, 1, open)).toBe(last);
  });

  test("skips the rows inside a collapsed directory", () => {
    const opts = { collapsed: new Set(["a/b"]) };
    expect(walk({ kind: "dir", id: "a" }, 1, opts)).toEqual(["a/b", 4, "ab", 1, 0]);
  });

  test("skips hidden files and the directories they empty", () => {
    const opts = { collapsed: new Set<string>(), isHidden: (i: number) => i === 1 || i === 3 };
    expect(walk({ kind: "dir", id: "a" }, 1, opts)).toEqual(["a/b", 2, 4, 0]);
  });

  test("moves on from a selection that is not shown", () => {
    const hidden = { collapsed: new Set(["a"]) };
    expect(stepSelection(nodes, { kind: "file", index: 3 }, 1, hidden)).toEqual({ kind: "dir", id: "ab" });
    expect(stepSelection(nodes, { kind: "file", index: 3 }, -1, hidden)).toEqual({ kind: "dir", id: "a" });
    const pruned = { collapsed: new Set<string>(), isHidden: (i: number) => i === 1 };
    expect(stepSelection(nodes, { kind: "file", index: 1 }, 1, pruned)).toEqual({ kind: "file", index: 0 });
  });
});
