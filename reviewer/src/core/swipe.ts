// Turns a one-finger touch into a file step (-1, 0, 1). This is the one place
// that owns how the swipe gesture feels, like `wheel.ts` for the wheel.
//
// A left swipe (finger moves left) steps forward, a right swipe steps back. A
// swipe must travel MIN_PX sideways and be more than twice as wide as it is
// tall, so vertical scrolling never steps. A horizontally scrolling diff pans
// first: a swipe steps only when, at its start, the diff was already at the
// edge the swipe pans toward. The caller reads the edges at the start, since
// the gesture itself pans the diff.

const MIN_PX = 50;

export interface SwipeEdges {
  atLeft: boolean;
  atRight: boolean;
}

export interface SwipeTracker {
  start(x: number, y: number, edges: SwipeEdges): void;
  cancel(): void;
  end(x: number, y: number): -1 | 0 | 1;
}

export function createSwipeTracker(): SwipeTracker {
  let origin: { x: number; y: number; edges: SwipeEdges } | null = null;
  return {
    start(x, y, edges) {
      origin = { x, y, edges };
    },
    cancel() {
      origin = null;
    },
    end(x, y) {
      const from = origin;
      origin = null;
      if (!from) return 0;
      const dx = x - from.x;
      const dy = y - from.y;
      if (Math.abs(dx) < MIN_PX || Math.abs(dx) <= 2 * Math.abs(dy)) return 0;
      if (dx < 0) return from.edges.atRight ? 1 : 0;
      return from.edges.atLeft ? -1 : 0;
    },
  };
}
