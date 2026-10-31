import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { initReviewCore } from "../src/core/reviewCore";

// `npm test` builds the core first (the `pretest` script). Vitest runs from
// reviewer/.
initReviewCore(new WebAssembly.Module(readFileSync(resolve("public/deltoids_review.wasm"))));
