import { describe, expect, test } from "vitest";
import { createSidesLoader, type SidesSource } from "./sidesLoader";
import { RateLimitError, type Sides } from "./github";

interface Pending {
  resolve(value: Sides | null): void;
  reject(err: Error): void;
}

// A source whose fetches stay pending until the test settles them.
function fakeSource(options: { remaining?: number | null } = {}) {
  const calls: number[] = [];
  const pending = new Map<number, Pending[]>();
  let remaining = options.remaining ?? null;
  const source: SidesSource = {
    fetch(index) {
      calls.push(index);
      return new Promise((resolve, reject) => {
        const list = pending.get(index) ?? [];
        list.push({ resolve, reject });
        pending.set(index, list);
      });
    },
    remaining: () => remaining,
  };
  const take = (index: number): Pending => {
    const list = pending.get(index);
    const next = list?.shift();
    if (!next) throw new Error(`no pending fetch for ${index}`);
    return next;
  };
  return {
    source,
    calls,
    inFlight: () => [...pending.values()].reduce((n, l) => n + l.length, 0),
    resolve: (index: number) => take(index).resolve(sides(index)),
    reject: (index: number, err: Error) => take(index).reject(err),
    setRemaining: (value: number | null) => {
      remaining = value;
    },
  };
}

function sides(index: number): Sides {
  return { kind: "full", before: "", after: `file ${index}`, path: `f${index}` };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("createSidesLoader", () => {
  test("fetches in the given order without exceeding the concurrency", async () => {
    const fake = fakeSource();
    createSidesLoader(fake.source, [3, 1, 2, 0, 4], { concurrency: 2 });
    await flush();
    expect(fake.calls).toEqual([3, 1]);

    fake.resolve(3);
    await flush();
    expect(fake.calls).toEqual([3, 1, 2]);
    expect(fake.inFlight()).toBe(2);
  });

  test("a load joins the background fetch and returns its result", async () => {
    const fake = fakeSource();
    const loader = createSidesLoader(fake.source, [0, 1], { concurrency: 2 });
    await flush();

    const first = loader.load(0);
    const second = loader.load(0);
    fake.resolve(0);

    expect(await first).toEqual(sides(0));
    expect(await second).toEqual(sides(0));
    expect(fake.calls.filter((i) => i === 0)).toHaveLength(1);
  });

  test("loading a queued file starts it at once and the queue skips it", async () => {
    const fake = fakeSource();
    const loader = createSidesLoader(fake.source, [0, 1, 2], { concurrency: 1 });
    await flush();
    expect(fake.calls).toEqual([0]);

    const loaded = loader.load(2);
    expect(fake.calls).toEqual([0, 2]);
    fake.resolve(2);
    expect(await loaded).toEqual(sides(2));

    fake.resolve(0);
    await flush();
    expect(fake.calls).toEqual([0, 2, 1]);
    fake.resolve(1);
    await flush();
    expect(fake.calls).toEqual([0, 2, 1]);
  });

  test("prioritize moves files ahead of the rest of the queue", async () => {
    const fake = fakeSource();
    const loader = createSidesLoader(fake.source, [0, 1, 2, 3, 4], { concurrency: 1 });
    await flush();

    loader.prioritize([4, 3]);
    fake.resolve(0);
    await flush();
    fake.resolve(4);
    await flush();
    fake.resolve(3);
    await flush();

    expect(fake.calls).toEqual([0, 4, 3, 1]);
  });

  test("prioritize ignores files already fetched or in flight", async () => {
    const fake = fakeSource();
    const loader = createSidesLoader(fake.source, [0, 1, 2], { concurrency: 1 });
    await flush();

    expect(() => loader.prioritize([0, 2])).not.toThrow();
    fake.resolve(0);
    await flush();
    expect(fake.calls).toEqual([0, 2]);
  });

  test("background stops at the rate-limit reserve but loads still fetch", async () => {
    const fake = fakeSource({ remaining: 50 });
    const loader = createSidesLoader(fake.source, [0, 1, 2], {
      concurrency: 1,
      reserve: 10,
    });
    await flush();
    fake.setRemaining(10);
    fake.resolve(0);
    await flush();
    expect(fake.calls).toEqual([0]);

    const loaded = loader.load(2);
    fake.resolve(2);
    expect(await loaded).toEqual(sides(2));
  });

  test("a rate-limit error stops the background queue", async () => {
    const fake = fakeSource();
    createSidesLoader(fake.source, [0, 1, 2], { concurrency: 1 });
    await flush();

    fake.reject(0, new RateLimitError("limited"));
    await flush();
    expect(fake.calls).toEqual([0]);
  });

  test("a background failure is retried by the next load", async () => {
    const fake = fakeSource();
    const loader = createSidesLoader(fake.source, [0], { concurrency: 1 });
    await flush();
    fake.reject(0, new Error("boom"));
    await flush();

    const loaded = loader.load(0);
    fake.resolve(0);
    expect(await loaded).toEqual(sides(0));
  });

  test("a load joining a background fetch that fails retries once", async () => {
    const fake = fakeSource();
    const loader = createSidesLoader(fake.source, [0], { concurrency: 1 });
    await flush();

    const loaded = loader.load(0);
    fake.reject(0, new Error("boom"));
    await flush();
    fake.resolve(0);
    expect(await loaded).toEqual(sides(0));
  });

  test("an on-demand failure rejects the load", async () => {
    const fake = fakeSource();
    const loader = createSidesLoader(fake.source, [], { concurrency: 1 });

    const loaded = loader.load(5);
    fake.reject(5, new Error("boom"));
    await expect(loaded).rejects.toThrow("boom");
  });

  test("whenFetched resolves when the background fetch lands, without fetching", async () => {
    const fake = fakeSource();
    const loader = createSidesLoader(fake.source, [0, 1], { concurrency: 1 });
    const fetched = loader.whenFetched(1);
    await flush();
    expect(fake.calls).toEqual([0]);

    fake.resolve(0);
    await flush();
    fake.resolve(1);
    expect(await fetched).toEqual(sides(1));
    expect(fake.calls).toEqual([0, 1]);
  });

  test("whenFetched resolves from an on-demand load", async () => {
    const fake = fakeSource();
    const loader = createSidesLoader(fake.source, [], { concurrency: 1 });
    const fetched = loader.whenFetched(3);

    void loader.load(3);
    fake.resolve(3);
    expect(await fetched).toEqual(sides(3));
  });

  test("whenFetched resolves after a failed background fetch is retried", async () => {
    const fake = fakeSource();
    const loader = createSidesLoader(fake.source, [0], { concurrency: 1 });
    const fetched = loader.whenFetched(0);
    await flush();
    fake.reject(0, new Error("boom"));
    await flush();

    void loader.load(0);
    fake.resolve(0);
    expect(await fetched).toEqual(sides(0));
  });

  test("dispose starts no further background fetches", async () => {
    const fake = fakeSource();
    const loader = createSidesLoader(fake.source, [0, 1, 2], { concurrency: 1 });
    await flush();

    loader.dispose();
    fake.resolve(0);
    await flush();
    expect(fake.calls).toEqual([0]);
  });
});
