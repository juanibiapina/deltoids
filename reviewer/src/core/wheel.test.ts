import { describe, expect, test } from "vitest";
import { createWheelStepper, type WheelInput } from "./wheel";

function wheel(deltaY: number, timeStamp: number, extra: Partial<WheelInput> = {}): WheelInput {
  return { deltaX: 0, deltaY, deltaMode: 0, timeStamp, ...extra };
}

describe("createWheelStepper", () => {
  test("every Shift+wheel tick steps, however small or fast", () => {
    // Firefox on macOS, Shift+wheel: one small, accelerating sideways event per tick.
    const s = createWheelStepper();
    const ticks: [number, number][] = [[-9, 1000], [-9, 1045], [-18, 1102], [-27, 1167], [-27, 1170]];
    expect(ticks.map(([dx, t]) => s.push(wheel(0, t, { deltaX: dx })))).toEqual([-1, -1, -1, -1, -1]);
  });

  test("vertical mouse ticks step once each", () => {
    const s = createWheelStepper();
    expect([0, 45, 100].map((t) => s.push(wheel(9, t)))).toEqual([1, 1, 1]);
  });

  test("a change of kind steps at once", () => {
    const s = createWheelStepper();
    expect(s.push(wheel(10, 0))).toBe(1);
    expect(s.push(wheel(0, 5, { deltaX: 10 }))).toBe(1);
    expect(s.push(wheel(10, 10))).toBe(1);
    expect(s.push(wheel(10, 15))).toBe(0);
  });

  test("a trackpad stream steps once per 40px, at most once per event", () => {
    const s = createWheelStepper();
    const steps = [0, 16, 32, 48, 64, 80, 96, 112, 128].map((t) => s.push(wheel(10, t)));
    expect(steps).toEqual([1, 0, 0, 0, 1, 0, 0, 0, 1]);
    expect([144, 160].map((t) => s.push(wheel(500, t)))).toEqual([1, 1]);
  });

  test("a direction change steps at once", () => {
    const s = createWheelStepper();
    expect(s.push(wheel(10, 0))).toBe(1);
    expect(s.push(wheel(-10, 16))).toBe(-1);
  });

  test("scales line-mode deltas to pixels", () => {
    const s = createWheelStepper();
    expect(s.push(wheel(1, 0, { deltaMode: 1 }))).toBe(1);
    expect(s.push(wheel(1, 10, { deltaMode: 1 }))).toBe(0);
    expect(s.push(wheel(1, 20, { deltaMode: 1 }))).toBe(1);
  });

  test("ignores zero deltas", () => {
    expect(createWheelStepper().push(wheel(0, 0))).toBe(0);
  });
});
