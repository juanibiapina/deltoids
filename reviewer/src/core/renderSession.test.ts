import { describe, expect, test, vi } from "vitest";
import { createRenderSession, type RenderSession } from "./renderSession";
import type { Engine } from "./engine";
import type { Sides } from "./github";
import type { SidesSource } from "./sidesLoader";

function sidesFor(index: number): Sides | null {
  if (index === 9) return null;
  return { kind: "full", before: "", after: `after${index}`, path: `f${index}` };
}

function fakeSource(): SidesSource & { fetch: ReturnType<typeof vi.fn> } {
  return {
    fetch: vi.fn((index: number) => Promise.resolve(sidesFor(index))),
    remaining: () => null,
  };
}

interface FakeEngine extends Engine {
  renders: ReturnType<typeof vi.fn>;
  closed: string[];
}

// Shapes render "path:theme", plus "+N" after N expansions.
function fakeEngine(onRender: (path: string) => void = () => {}): FakeEngine {
  const renders = vi.fn();
  const closed: string[] = [];
  return {
    renders,
    closed,
    openShape: (sides) => {
      let level = 0;
      return {
        render: (theme) => {
          renders(sides.path, theme);
          onRender(sides.path);
          if (sides.path === "f8") return "";
          return `${sides.path}:${theme}${level ? `+${level}` : ""}`;
        },
        expand: () => {
          level++;
          return true;
        },
        shrink: () => {
          if (level === 0) return false;
          level--;
          return true;
        },
        close: () => closed.push(sides.path),
      };
    },
    renderContext: vi.fn(
      (after: string, path: string, start: number, end: number, theme: string) =>
        `${after}|${path}|${start}-${end}|${theme}`,
    ),
  };
}

function session(order: number[], engine: Engine, source = fakeSource()) {
  return { source, session: createRenderSession(source, order, Promise.resolve(engine)) };
}

describe("RenderSession", () => {
  test("renders html, binary, and empty results", async () => {
    const { session: s } = session([], fakeEngine());
    await expect(s.render(0, "T", "now")).resolves.toEqual({ kind: "html", html: "f0:T" });
    await expect(s.render(9, "T", "now")).resolves.toEqual({ kind: "binary" });
    await expect(s.render(8, "T", "now")).resolves.toEqual({ kind: "empty" });
  });

  test("a background render waits for the background fetch; now starts one", async () => {
    const { session: s, source } = session([], fakeEngine());
    const background = s.render(0, "T", "background");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(source.fetch).not.toHaveBeenCalled();

    await expect(s.render(0, "T", "now")).resolves.toEqual({ kind: "html", html: "f0:T" });
    await expect(background).resolves.toEqual({ kind: "html", html: "f0:T" });
    expect(source.fetch).toHaveBeenCalledTimes(1);
  });

  test("an urgent render that arrives mid-render runs before queued background work", async () => {
    const rendered: string[] = [];
    let s: RenderSession | null = null;
    let urgent: Promise<unknown> | null = null;
    const engine = fakeEngine((path) => {
      rendered.push(path);
      // Arrives like a worker message: a macrotask queued during a wasm call.
      if (path === "f0") setTimeout(() => (urgent = s!.render(3, "T", "now")));
    });
    s = session([0, 1, 2, 3], engine).session;
    const background = [0, 1, 2].map((i) => s!.render(i, "T", "background"));
    await Promise.all(background);
    await urgent;
    expect(rendered).toEqual(["f0", "f3", "f1", "f2"]);
  });

  test("a theme switch re-renders from cached content without re-fetching", async () => {
    const engine = fakeEngine();
    const { session: s, source } = session([], engine);
    await s.render(0, "A", "now");
    await s.render(0, "A", "now");
    expect(engine.renders).toHaveBeenCalledTimes(1);

    await expect(s.render(0, "B", "now")).resolves.toEqual({ kind: "html", html: "f0:B" });
    expect(engine.renders).toHaveBeenCalledTimes(2);
    expect(source.fetch).toHaveBeenCalledTimes(1);
  });

  test("a request for another theme supersedes a pending one", async () => {
    const { session: s } = session([], fakeEngine());
    const old = s.render(0, "A", "background");
    await expect(s.render(0, "B", "now")).resolves.toEqual({ kind: "html", html: "f0:B" });
    await expect(old).rejects.toThrow("superseded");
  });

  test("context rows render from the file's fetched content", async () => {
    const { session: s, source } = session([], fakeEngine());
    await s.render(0, "T", "now");
    await expect(s.renderContext(0, 2, 5, "T")).resolves.toBe("after0|f0|2-5|T");
    expect(source.fetch).toHaveBeenCalledTimes(1);
  });

  test("a reshape re-renders the file, and later renders keep the shape", async () => {
    const engine = fakeEngine();
    const { session: s, source } = session([], engine);
    await s.render(0, "A", "now");
    await expect(s.reshape(0, "expand", 0, "A")).resolves.toEqual({ kind: "html", html: "f0:A+1" });
    await expect(s.render(0, "A", "now")).resolves.toEqual({ kind: "html", html: "f0:A+1" });
    await expect(s.render(0, "B", "now")).resolves.toEqual({ kind: "html", html: "f0:B+1" });
    await expect(s.reshape(0, "shrink", 0, "B")).resolves.toEqual({ kind: "html", html: "f0:B" });
    expect(source.fetch).toHaveBeenCalledTimes(1);
  });

  test("a reshape that changes nothing, or comes before the first render, is null", async () => {
    const { session: s } = session([], fakeEngine());
    await expect(s.reshape(0, "expand", 0, "A")).resolves.toBeNull();
    await s.render(0, "A", "now");
    await expect(s.reshape(0, "shrink", 0, "A")).resolves.toBeNull();
  });

  test("dispose closes every open shape", async () => {
    const engine = fakeEngine();
    const { session: s } = session([], engine);
    await s.render(0, "A", "now");
    await s.render(1, "A", "now");
    s.dispose();
    expect(engine.closed.sort()).toEqual(["f0", "f1"]);
  });
});
