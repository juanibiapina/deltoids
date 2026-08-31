import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useRef,
  type ReactNode,
} from "react";

// A single shared IntersectionObserver for all file cards, so a large PR does
// not create thousands of observers. Cards register their element plus a
// one-shot load callback; the observer fires it once the card nears the
// viewport (600px rootMargin), then unobserves it.

interface LazyRegistry {
  observe(el: Element, cb: () => void): void;
  unobserve(el: Element): void;
}

const LazyContext = createContext<LazyRegistry | null>(null);

export function LazyObserverProvider({ children }: { children: ReactNode }) {
  const callbacks = useRef(new Map<Element, () => void>());

  const observer = useMemo(() => {
    if (typeof IntersectionObserver === "undefined") return null;
    return new IntersectionObserver(
      (entries, obs) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          const cb = callbacks.current.get(entry.target);
          obs.unobserve(entry.target);
          callbacks.current.delete(entry.target);
          if (cb) cb();
        }
      },
      { rootMargin: "600px 0px" },
    );
  }, []);

  useEffect(() => () => observer?.disconnect(), [observer]);

  const registry = useMemo<LazyRegistry>(
    () => ({
      observe(el, cb) {
        if (!observer) {
          // No IntersectionObserver (e.g. jsdom): load eagerly.
          cb();
          return;
        }
        callbacks.current.set(el, cb);
        observer.observe(el);
      },
      unobserve(el) {
        callbacks.current.delete(el);
        observer?.unobserve(el);
      },
    }),
    [observer],
  );

  return (
    <LazyContext.Provider value={registry}>{children}</LazyContext.Provider>
  );
}

export function useLazy(): LazyRegistry {
  const ctx = useContext(LazyContext);
  if (!ctx) throw new Error("useLazy must be used within LazyObserverProvider");
  return ctx;
}
