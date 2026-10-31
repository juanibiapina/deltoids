import * as Comlink from "comlink";
import { instantiateEngine, type Engine } from "./engine";
import { githubSidesSource, resetRateRemaining } from "./github";
import { createRenderSession } from "./renderSession";
import type { OpenReview } from "./reviewClient";

// The page posts the compiled module once compilation finishes, which can be
// after a review is opened; sessions wait on this promise until then.
let provideModule: (module: WebAssembly.Module) => void = () => {};
const engine: Promise<Engine> = new Promise<WebAssembly.Module>((resolve) => {
  provideModule = resolve;
}).then(instantiateEngine);

// Whether GitHub requests go through the Worker (signed in). One flag for the
// whole worker; open sessions read it on every request, so a sign-in change
// reaches them without reopening.
let viaWorker = false;

const api = {
  init(module: WebAssembly.Module): Promise<void> {
    provideModule(module);
    return engine.then(() => {});
  },
  setSignedIn(value: boolean): void {
    viaWorker = value;
    // The last seen budget belongs to the previous identity; keeping it could
    // stop a new session's prefetch before its first request.
    resetRateRemaining();
  },
  open({ ref, files, baseSha, headSha, order }: OpenReview) {
    const source = githubSidesSource(ref, files, baseSha, headSha, () => viaWorker);
    const session = createRenderSession(source, order, engine);
    return Comlink.proxy({
      render: session.render,
      renderContext: session.renderContext,
      reshape: session.reshape,
      prioritize: session.prioritize,
      dispose: session.dispose,
    });
  },
};

export type ReviewWorkerApi = typeof api;

Comlink.expose(api);
