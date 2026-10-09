import PQueue from "p-queue";
import { RateLimitError, type Sides } from "./github";

export interface SidesSource {
  fetch(index: number): Promise<Sides | null>;
  remaining(): number | null;
}

export interface SidesLoader {
  load(index: number): Promise<Sides | null>;
  whenFetched(index: number): Promise<Sides | null>;
  prioritize(indices: number[]): void;
  dispose(): void;
}

export interface SidesLoaderOptions {
  concurrency?: number;
  reserve?: number;
}

export function createSidesLoader(
  source: SidesSource,
  order: number[],
  { concurrency = 4, reserve = 10 }: SidesLoaderOptions = {},
): SidesLoader {
  const cache = new Map<number, Promise<Sides | null>>();
  const background = new WeakSet<Promise<Sides | null>>();
  const queued = new Set<number>();
  const queue = new PQueue({ concurrency });
  let stopped = false;
  let nextPriority = 1;
  const waiters = new Map<number, Array<(sides: Sides | null) => void>>();

  const track = (index: number, promise: Promise<Sides | null>) => {
    cache.set(index, promise);
    promise.then(
      (sides) => {
        waiters.get(index)?.forEach((resolve) => resolve(sides));
        waiters.delete(index);
      },
      () => {},
    );
  };

  const stop = () => {
    stopped = true;
    queue.clear();
    queued.clear();
  };

  const fetchInBackground = async (index: number) => {
    queued.delete(index);
    if (stopped || cache.has(index)) return;
    const remaining = source.remaining();
    if (remaining !== null && remaining <= reserve) {
      stop();
      return;
    }
    const promise = source.fetch(index);
    background.add(promise);
    track(index, promise);
    try {
      await promise;
    } catch (err) {
      if (cache.get(index) === promise) cache.delete(index);
      if (err instanceof RateLimitError) stop();
    }
  };

  for (const index of order) {
    if (queued.has(index)) continue;
    queued.add(index);
    void queue.add(() => fetchInBackground(index), { id: String(index), priority: 0 });
  }

  return {
    load(index) {
      const existing = cache.get(index);
      if (existing && !background.has(existing)) return existing;
      const promise = existing
        ? existing.catch(() => source.fetch(index))
        : source.fetch(index);
      track(index, promise);
      return promise;
    },
    whenFetched(index) {
      return new Promise((resolve) => {
        const list = waiters.get(index) ?? [];
        list.push(resolve);
        waiters.set(index, list);
        cache.get(index)?.then(resolve, () => {});
      });
    },
    prioritize(indices) {
      const waiting = indices.filter((i) => queued.has(i));
      waiting.forEach((index, k) => {
        queue.setPriority(String(index), nextPriority + waiting.length - k);
      });
      nextPriority += waiting.length + 1;
    },
    dispose: stop,
  };
}
