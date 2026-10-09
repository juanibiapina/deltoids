import * as Comlink from "comlink";
import { compileEngine } from "./engine";
import type { PrFile } from "./github";
import type { PrRef } from "./lib";
import type { RenderSession } from "./renderSession";
import type { ReviewWorkerApi } from "./review.worker";

export interface OpenReview {
  ref: PrRef;
  files: PrFile[];
  baseSha: string;
  headSha: string;
  order: number[];
  token: string;
}

export interface ReviewSession extends RenderSession {
  setToken(token: string): void;
}

interface ReviewWorker {
  api: Comlink.Remote<ReviewWorkerApi>;
  ready: Promise<void>;
}

let worker: ReviewWorker | null = null;

// Start the review worker and the engine once per page. The main thread
// compiles the preloaded wasm and posts the module to the worker, which
// instantiates it. `ready` rejects when either step fails.
export function startReviewWorker(): ReviewWorker {
  if (!worker) {
    const api = Comlink.wrap<ReviewWorkerApi>(
      new Worker(new URL("./review.worker.ts", import.meta.url), {
        type: "module",
      }),
    );
    const ready = compileEngine().then((module) => api.init(module));
    ready.catch(() => {});
    worker = { api, ready };
  }
  return worker;
}

// Open one review's session in the worker. Rendering waits for the engine;
// await `startReviewWorker().ready` to surface engine errors.
export async function openReview(review: OpenReview): Promise<ReviewSession> {
  const remote = await startReviewWorker().api.open(review);
  return {
    render: (index, theme, urgency) => remote.render(index, theme, urgency),
    renderContext: (index, start, end, theme) =>
      remote.renderContext(index, start, end, theme),
    prioritize: (indices) => void remote.prioritize(indices),
    setToken: (token) => void remote.setToken(token),
    dispose: () => {
      void remote.dispose().finally(() => remote[Comlink.releaseProxy]());
    },
  };
}
