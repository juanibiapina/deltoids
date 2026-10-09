import { describe, expect, test } from "vitest";
import { whenIdle } from "./idle";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("whenIdle", () => {
  test("runs one task per idle slot, in order", async () => {
    const ran: number[] = [];
    whenIdle(() => ran.push(1));
    whenIdle(() => ran.push(2));
    whenIdle(() => ran.push(3));
    expect(ran).toEqual([]);

    await tick();
    expect(ran).toEqual([1]);
    await tick();
    await tick();
    expect(ran).toEqual([1, 2, 3]);
  });

  test("a cancelled task never runs", async () => {
    const ran: string[] = [];
    const cancel = whenIdle(() => ran.push("a"));
    whenIdle(() => ran.push("b"));
    cancel();

    await tick();
    await tick();
    expect(ran).toEqual(["b"]);
  });
});
