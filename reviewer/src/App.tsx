import { useCallback, useEffect, useRef, useState } from "react";
import { loadEngine } from "./core/engine";
import { fetchPr, fetchFiles, token, setToken } from "./core/github";
import { parsePrUrl } from "./core/lib";
import { usePrefs } from "./hooks/usePrefs";
import { useTopbarHeight } from "./hooks/useTopbarHeight";
import { Topbar } from "./components/Topbar";
import { ReviewView, type ReviewData } from "./components/ReviewView";

interface Status {
  text: string;
  isError: boolean;
}

export function App() {
  const topbarRef = useRef<HTMLElement>(null);
  useTopbarHeight(topbarRef);

  const prefs = usePrefs();

  const [input, setInput] = useState("");
  const [status, setStatus] = useState<Status>({ text: "", isError: false });
  const [data, setData] = useState<ReviewData | null>(null);
  const [started, setStarted] = useState(false);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [hasToken, setHasToken] = useState(() => Boolean(token()));

  // Guards against overlapping reviews: only the latest request may write state.
  const requestId = useRef(0);

  const closeDrawer = useCallback(() => setDrawerOpen(false), []);

  // Mobile drawer: reflect open state on <body>, close on Escape.
  useEffect(() => {
    document.body.classList.toggle("drawer-open", drawerOpen);
  }, [drawerOpen]);

  // Reflect the theme choice on <html> so the CSS palette switches.
  useEffect(() => {
    document.documentElement.dataset.theme = prefs.theme;
  }, [prefs.theme]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeDrawer();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [closeDrawer]);

  const review = useCallback(async (value: string) => {
    const ref = parsePrUrl(value);
    if (!ref) {
      setData(null);
      setStatus({
        text: "Enter a GitHub PR URL like https://github.com/owner/repo/pull/123",
        isError: true,
      });
      return;
    }

    const id = ++requestId.current;
    setStarted(true);
    setData(null);
    setDrawerOpen(false);
    setStatus({ text: "Loading engine and PR…", isError: false });

    try {
      const [engine, pr, files] = await Promise.all([
        loadEngine(),
        fetchPr(ref),
        fetchFiles(ref),
      ]);
      if (id !== requestId.current) return;
      setData({ ref, pr, files, engine, baseSha: pr.base.sha, headSha: pr.head.sha });
      setStatus({ text: `${files.length} files. Scroll to load.`, isError: false });
    } catch (err) {
      if (id !== requestId.current) return;
      const e = err as Error & { detail?: string };
      setStatus({ text: e.message, isError: true });
      if (e.detail) console.error(e.detail);
    }
  }, []);

  const submit = useCallback(
    (value: string) => {
      const params = new URLSearchParams(location.search);
      params.set("pr", value);
      history.replaceState(null, "", `?${params.toString()}`);
      void review(value);
    },
    [review],
  );

  // Deep link: ?pr=<url or owner/repo/number>. Runs once on mount.
  useEffect(() => {
    const initial = new URLSearchParams(location.search).get("pr");
    if (initial) {
      setInput(initial);
      void review(initial);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const onToken = useCallback(() => {
    const current = token();
    const next = prompt(
      "GitHub personal access token (read-only, stored in this browser). Leave blank to clear.",
      current,
    );
    if (next === null) return;
    setToken(next);
    setHasToken(Boolean(next.trim()));
    setStatus({
      text: next.trim() ? "Token saved." : "Token cleared.",
      isError: false,
    });
  }, []);

  const onProgress = useCallback((loaded: number, total: number) => {
    setStatus({ text: `Loaded ${loaded}/${total} files.`, isError: false });
  }, []);

  return (
    <>
      <Topbar
        topbarRef={topbarRef}
        input={input}
        onInput={setInput}
        onSubmit={() => submit(input)}
        hasToken={hasToken}
        onToken={onToken}
        showToolbar={started}
        prefs={prefs}
        onFilesToggle={() => setDrawerOpen((v) => !v)}
        drawerOpen={drawerOpen}
      />

      <div
        className="scrim"
        hidden={!drawerOpen}
        onClick={closeDrawer}
        aria-hidden="true"
      ></div>

      <div className={`status${status.isError ? " error" : ""}`} role="status" aria-live="polite">
        {status.text}
      </div>

      <main id="app" className={prefs.nowrap ? "nowrap" : undefined} data-size={prefs.size}>
        {data && (
          <ReviewView
            data={data}
            syntaxTheme={prefs.syntaxTheme}
            onNavigate={closeDrawer}
            onProgress={onProgress}
          />
        )}
      </main>
    </>
  );
}
