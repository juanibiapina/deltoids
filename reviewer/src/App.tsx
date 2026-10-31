import { useCallback, useEffect, useRef, useState } from "react";
import { fetchPr, fetchFiles } from "./core/github";
import { parsePrUrl } from "./core/lib";
import { buildTree, displayOrder } from "./core/filetree";
import { loadReviewCore } from "./core/reviewCore";
import { attachSheetDrag } from "./core/sheetDrag";
import { openReview, setWorkerSignedIn, startReviewWorker } from "./core/reviewClient";
import type { RenderSession } from "./core/renderSession";
import { usePrefs } from "./hooks/usePrefs";
import { useTopbarHeight } from "./hooks/useTopbarHeight";
import { useMediaQuery } from "./hooks/useMediaQuery";
import { Topbar } from "./components/Topbar";
import { ReviewView, type ReviewData } from "./components/ReviewView";
import { useGitHubAccess } from "./components/Account";
import { PullRequestList } from "./components/PullRequestList";

interface Status {
  text: string;
  isError: boolean;
}

export function App() {
  const topbarRef = useRef<HTMLElement>(null);
  useTopbarHeight(topbarRef);

  const prefs = usePrefs();
  const { loaded, signedIn } = useGitHubAccess();
  const smallTouch = useMediaQuery("(max-width: 767px) and (pointer: coarse)");
  const layout =
    prefs.layoutChoice === "auto" ? (smallTouch ? "phone" : "desktop") : prefs.layoutChoice;

  const [input, setInput] = useState("");
  const [status, setStatus] = useState<Status>({ text: "", isError: false });
  const [data, setData] = useState<ReviewData | null>(null);
  const [started, setStarted] = useState(false);
  const [drawerOpen, setDrawerOpen] = useState(false);
  // The deep link, read once. Decided during render so the home list never
  // mounts (and searches) on a deep link: child effects run before App's.
  const [initialPr] = useState(() => new URLSearchParams(location.search).get("pr"));

  // Guards against overlapping reviews: only the latest request may write state.
  const requestId = useRef(0);
  const sessionRef = useRef<RenderSession | null>(null);

  useEffect(() => {
    startReviewWorker();
  }, []);

  // The only path that sets the review worker's flag. It runs in the effect
  // flush, before any `openReview` (which waits on fetchPr/fetchFiles), so a
  // new session starts with the current value; open sessions pick up changes.
  useEffect(() => {
    setWorkerSignedIn(signedIn);
  }, [signedIn]);

  const closeDrawer = useCallback(() => setDrawerOpen(false), []);
  const openDrawer = useCallback(() => setDrawerOpen(true), []);

  // Mobile drawer: reflect open state on <body>, close on Escape.
  useEffect(() => {
    document.body.classList.toggle("drawer-open", drawerOpen);
  }, [drawerOpen]);

  useEffect(() => {
    document.documentElement.classList.toggle("reviewing", data !== null);
  }, [data]);

  useEffect(() => {
    document.documentElement.dataset.layout = layout;
  }, [layout]);

  // On phones the file list is a full-height sheet: drag it down to close.
  useEffect(() => {
    if (layout !== "phone" || !drawerOpen) return;
    const sheet = document.querySelector<HTMLElement>(".sidebar");
    return sheet ? attachSheetDrag(sheet, closeDrawer) : undefined;
  }, [layout, drawerOpen, closeDrawer]);

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

  const review = useCallback(async (value: string, viaWorker: boolean) => {
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
    sessionRef.current?.dispose();
    sessionRef.current = null;
    setStarted(true);
    setData(null);
    setDrawerOpen(false);
    setStatus({ text: "Loading engine and PR…", isError: false });

    try {
      const { ready } = startReviewWorker();
      const [pr, files] = await Promise.all([
        fetchPr(ref, viaWorker),
        fetchFiles(ref, viaWorker),
        loadReviewCore(),
      ]);
      if (id !== requestId.current) return;
      const order = displayOrder(
        buildTree(files.map((f) => ({ filename: f.filename, status: f.status }))),
      );
      const session = await openReview({
        ref,
        files,
        baseSha: pr.base.sha,
        headSha: pr.head.sha,
        order,
      });
      if (id !== requestId.current) {
        session.dispose();
        return;
      }
      sessionRef.current = session;
      await ready;
      if (id !== requestId.current) return;
      setData({ ref, pr, files, session });
      setStatus({ text: "", isError: false });
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
      void review(value, signedIn);
    },
    [review, signedIn],
  );

  // Deep link: ?pr=<url or owner/repo/number>. Waits for Clerk, so a
  // signed-in user's first request does not go out anonymously, then runs
  // once: `loaded` never returns to false, and sign-in changes do not re-run it.
  useEffect(() => {
    if (!loaded) return;
    if (initialPr) {
      setInput(initialPr);
      void review(initialPr, signedIn);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loaded]);

  const openFromList = useCallback(
    (url: string) => {
      setInput(url);
      submit(url);
    },
    [submit],
  );

  return (
    <>
      <Topbar
        topbarRef={topbarRef}
        input={input}
        onInput={setInput}
        onSubmit={() => submit(input)}
        ready={loaded}
        started={started}
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

      <main
        id="app"
        className={
          [
            prefs.nowrap && "nowrap",
            prefs.hideLineNumbers && "hide-ln",
            prefs.hideViewed && "hide-viewed",
          ]
            .filter(Boolean)
            .join(" ") || undefined
        }
        data-size={prefs.size}
      >
        {!initialPr && loaded && signedIn && !started && (
          <PullRequestList onOpen={openFromList} />
        )}
        {data && (
          <ReviewView
            data={data}
            syntaxTheme={prefs.syntaxTheme}
            hideViewed={prefs.hideViewed}
            layout={layout}
            onNavigate={closeDrawer}
            onShowTree={openDrawer}
          />
        )}
      </main>
    </>
  );
}
