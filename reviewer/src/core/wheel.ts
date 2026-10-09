// Turns modified wheel events into sidebar steps (-1, 0, 1). This is the one
// place that owns how the step gesture feels. It mirrors the TUI's
// `crates/deltoids-cli/src/cli/browse/scroll.rs`, which sorts wheel events
// into two kinds:
//
// - Discrete: macOS delivers Shift+wheel as horizontal events, one per tick.
//   Every such event is one step, at any speed (the TUI's `Discrete`).
// - List: vertical events. The terminal turns a tick into a burst of about 3
//   events and steps once per burst (the TUI's `List`). A browser sends one
//   event per tick instead, so an event at least TICK_GAP_MS after the last
//   is one step. A trackpad streams events every frame; within a stream,
//   travel steps once per STEP_PX, at most once per event.
//
// Like the TUI, a change of direction or kind steps at once.

const STEP_PX = 40;
const TICK_GAP_MS = 30;
const LINE_PX = 20;
const PAGE_PX = 400;

export interface WheelInput {
  deltaX: number;
  deltaY: number;
  deltaMode: number;
  timeStamp: number;
}

export type Step = -1 | 0 | 1;

type Kind = "discrete" | "list";

function pixels(delta: number, deltaMode: number): number {
  if (deltaMode === 1) return delta * LINE_PX;
  if (deltaMode === 2) return delta * PAGE_PX;
  return delta;
}

export function createWheelStepper(): { push(e: WheelInput): Step } {
  let gesture: { sign: Step; kind: Kind } | null = null;
  let lastTime = -Infinity;
  let travel = 0;
  return {
    push(e) {
      const kind: Kind = e.deltaY === 0 ? "discrete" : "list";
      const delta = pixels(kind === "discrete" ? e.deltaX : e.deltaY, e.deltaMode);
      if (delta === 0) return 0;
      const sign: Step = delta > 0 ? 1 : -1;
      const gap = e.timeStamp - lastTime;
      lastTime = e.timeStamp;
      const fresh = gesture?.sign !== sign || gesture.kind !== kind;
      gesture = { sign, kind };
      if (fresh || kind === "discrete" || gap >= TICK_GAP_MS) {
        travel = 0;
        return sign;
      }
      travel += Math.abs(delta);
      if (travel < STEP_PX) return 0;
      travel %= STEP_PX;
      return sign;
    },
  };
}
