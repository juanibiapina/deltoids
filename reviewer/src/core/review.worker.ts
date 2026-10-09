import * as Comlink from "comlink";
import { instantiateEngine, type Engine } from "./engine";
import { githubSidesSource } from "./github";
import { createRenderSession } from "./renderSession";
import type { OpenReview } from "./reviewClient";

// The page posts the compiled module once compilation finishes, which can be
// after a review is opened; sessions wait on this promise until then.
let provideModule: (module: WebAssembly.Module) => void = () => {};
const engine: Promise<Engine> = new Promise<WebAssembly.Module>((resolve) => {
  provideModule = resolve;
}).then(instantiateEngine);

const api = {
  init(module: WebAssembly.Module): Promise<void> {
    provideModule(module);
    return engine.then(() => {});
  },
  open({ ref, files, baseSha, headSha, order, token }: OpenReview) {
    let auth = token;
    const source = githubSidesSource(ref, files, baseSha, headSha, () => auth);
    const session = createRenderSession(source, order, engine);
    return Comlink.proxy({
      render: session.render,
      renderContext: session.renderContext,
      prioritize: session.prioritize,
      dispose: session.dispose,
      setToken(next: string) {
        auth = next;
      },
    });
  },
};

export type ReviewWorkerApi = typeof api;

Comlink.expose(api);
