import { describe, expect, test, vi } from "vitest";
import { renderSides, type Sides } from "./github";
import type { Engine } from "./engine";

describe("renderSides", () => {
  test("full sides call renderFile with the theme", () => {
    const engine = {
      renderFile: vi.fn().mockReturnValue("<full>"),
      renderFromPatch: vi.fn(),
    } satisfies Engine;
    const sides: Sides = { kind: "full", before: "a", after: "b", path: "x.rs" };

    const html = renderSides(engine, sides, "TokyoNight");

    expect(html).toBe("<full>");
    expect(engine.renderFile).toHaveBeenCalledWith("a", "b", "x.rs", "TokyoNight");
    expect(engine.renderFromPatch).not.toHaveBeenCalled();
  });

  test("patch sides call renderFromPatch with the theme", () => {
    const engine = {
      renderFile: vi.fn(),
      renderFromPatch: vi.fn().mockReturnValue("<patch>"),
    } satisfies Engine;
    const sides: Sides = {
      kind: "patch",
      after: "b",
      patch: "@@ -1 +1 @@",
      path: "x.rs",
    };

    const html = renderSides(engine, sides, "GitHub");

    expect(html).toBe("<patch>");
    expect(engine.renderFromPatch).toHaveBeenCalledWith(
      "b",
      "@@ -1 +1 @@",
      "x.rs",
      "GitHub",
    );
    expect(engine.renderFile).not.toHaveBeenCalled();
  });

  test("re-rendering the same sides with a new theme hits no network", () => {
    const engine = {
      renderFile: vi.fn().mockReturnValue("<html>"),
      renderFromPatch: vi.fn(),
    } satisfies Engine;
    const sides: Sides = { kind: "full", before: "a", after: "b", path: "x.rs" };

    renderSides(engine, sides, "TokyoNight");
    renderSides(engine, sides, "GitHub");

    // Two pure renders from one cached Sides; the second passes the new theme.
    expect(engine.renderFile).toHaveBeenNthCalledWith(1, "a", "b", "x.rs", "TokyoNight");
    expect(engine.renderFile).toHaveBeenNthCalledWith(2, "a", "b", "x.rs", "GitHub");
  });
});
