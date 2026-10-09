import PQueue from "p-queue";
import { renderSides, type Sides } from "./github";
import type { Engine } from "./engine";
import { createSidesLoader, type SidesSource } from "./sidesLoader";

export type Rendered =
  | { kind: "html"; html: string }
  | { kind: "binary" }
  | { kind: "empty" };

export type Urgency = "now" | "background";

// One review's files: fetching, the wasm renders, and a one-theme HTML cache
// per file. Runs inside the review worker; the main thread sees it through
// Comlink, so every method is async from there.
export interface RenderSession {
  // "now" fetches at once if needed and renders ahead of background work.
  // "background" waits for the background fetch (never starts one) and
  // renders when nothing urgent is queued. A later "now" for the same file
  // and theme promotes the pending request. Rejects with the fetch or render
  // error, or when a request for another theme supersedes it.
  render(index: number, theme: string, urgency: Urgency): Promise<Rendered>;
  // Highlighted context rows for new-file lines start..=end of file `index`.
  renderContext(
    index: number,
    start: number,
    end: number,
    theme: string,
  ): Promise<string>;
  prioritize(indices: number[]): void;
  dispose(): void;
}

const BACKGROUND = 0;
const NOW = 1;
const CONTEXT = 2;

interface Job {
  theme: string;
  result: Promise<Rendered>;
  promote(): void;
  cancel(): void;
}

function nextTask(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

export function createRenderSession(
  source: SidesSource,
  order: number[],
  engine: Promise<Engine>,
): RenderSession {
  const loader = createSidesLoader(source, order);
  const queue = new PQueue({ concurrency: 1 });
  const jobs = new Map<number, Job>();
  let nextId = 0;

  // Wasm calls are synchronous. Yielding a macrotask after each one lets
  // messages that arrived meanwhile (a click's "now" render) enqueue before
  // the queue picks its next task; a microtask chain would starve them.
  const run = <T>(
    work: (engine: Engine) => T,
    priority: number,
    id = String(nextId++),
  ): Promise<T> =>
    queue.add(
      async () => {
        const ready = await engine;
        try {
          return work(ready);
        } finally {
          await nextTask();
        }
      },
      { priority, id },
    ) as Promise<T>;

  const startJob = (index: number, theme: string, urgent: boolean): Job => {
    let isUrgent = urgent;
    let cancelled = false;
    let queued = false;
    const taskId = String(nextId++);
    let fetchNow = () => {};
    const sides = new Promise<Sides | null>((resolve, reject) => {
      void loader.whenFetched(index).then(resolve);
      fetchNow = () => loader.load(index).then(resolve, reject);
    });
    if (urgent) fetchNow();

    const result = sides.then((fetched): Promise<Rendered> | Rendered => {
      if (cancelled) throw new Error("superseded");
      if (fetched === null) return { kind: "binary" };
      queued = true;
      return run(
        (ready): Rendered => {
          if (cancelled) throw new Error("superseded");
          const html = renderSides(ready, fetched, theme);
          return html ? { kind: "html", html } : { kind: "empty" };
        },
        isUrgent ? NOW : BACKGROUND,
        taskId,
      );
    });

    const job: Job = {
      theme,
      result,
      promote() {
        if (isUrgent) return;
        isUrgent = true;
        fetchNow();
        if (!queued) return;
        try {
          queue.setPriority(taskId, NOW);
        } catch {
          // Already running or done.
        }
      },
      cancel() {
        cancelled = true;
      },
    };
    result.catch(() => {
      if (jobs.get(index) === job) jobs.delete(index);
    });
    return job;
  };

  return {
    render(index, theme, urgency) {
      const existing = jobs.get(index);
      if (existing?.theme === theme) {
        if (urgency === "now") existing.promote();
        return existing.result;
      }
      existing?.cancel();
      const job = startJob(index, theme, urgency === "now");
      jobs.set(index, job);
      return job.result;
    },
    async renderContext(index, start, end, theme) {
      const sides = await loader.load(index);
      if (!sides) return "";
      return run(
        (ready) => ready.renderContext(sides.after, sides.path, start, end, theme),
        CONTEXT,
      );
    },
    prioritize(indices) {
      loader.prioritize(indices);
    },
    dispose() {
      loader.dispose();
      queue.clear();
      jobs.forEach((job) => job.cancel());
      jobs.clear();
    },
  };
}
