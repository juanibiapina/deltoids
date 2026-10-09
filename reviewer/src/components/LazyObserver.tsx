import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useRef,
  type ReactNode,
  type RefObject,
} from "react";

// A single shared IntersectionObserver for all file cards, so a large PR does
// not create thousands of observers. Cards register their element plus a
// one-shot load callback; the observer fires it once the card nears the
// scroll container (600px rootMargin), then unobserves it. The observer is
// created on the first registration, after React has attached `rootRef`.

interface LazyRegistry {
  observe(el: Element, cb: () => void): void;
  unobserve(el: Element): void;
}

const LazyContext = createContext<LazyRegistry | null>(null);

export function LazyObserverProvider({
  rootRef,
  children,
}: {
  rootRef?: RefObject<Element | null>;
  children: ReactNode;
}) {
  const callbacks = useRef(new Map<Element, () => void>());
  const observer = useRef<IntersectionObserver | null>(null);

  useEffect(() => () => observer.current?.disconnect(), []);

  const registry = useMemo<LazyRegistry>(() => {
    const ensure = (): IntersectionObserver | null => {
      if (typeof IntersectionObserver === "undefined") return null;
      observer.current ??= new IntersectionObserver(
        (entries, obs) => {
          for (const entry of entries) {
            if (!entry.isIntersecting) continue;
            const cb = callbacks.current.get(entry.target);
            obs.unobserve(entry.target);
            callbacks.current.delete(entry.target);
            if (cb) cb();
          }
        },
        { root: rootRef?.current ?? null, rootMargin: "600px 0px" },
      );
      return observer.current;
    };
    return {
      observe(el, cb) {
        const obs = ensure();
        if (!obs) {
          cb();
          return;
        }
        callbacks.current.set(el, cb);
        obs.observe(el);
      },
      unobserve(el) {
        callbacks.current.delete(el);
        observer.current?.unobserve(el);
      },
    };
  }, [rootRef]);

  return (
    <LazyContext.Provider value={registry}>{children}</LazyContext.Provider>
  );
}

export function useLazy(): LazyRegistry {
  const ctx = useContext(LazyContext);
  if (!ctx) throw new Error("useLazy must be used within LazyObserverProvider");
  return ctx;
}
