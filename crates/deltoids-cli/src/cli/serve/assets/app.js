"use strict";

// deltoids trace reviewer — a tiny hand-rolled SPA. Three views:
// projects -> traces -> reviewer (swipe through a trace's edits).

const view = document.getElementById("view");
const titleEl = document.getElementById("title");
const subtitleEl = document.getElementById("subtitle");
const counterEl = document.getElementById("counter");
const backEl = document.getElementById("back");
const toastEl = document.getElementById("toast");

const state = {
  where: "projects", // projects | traces | reviewer
  project: null, // { id, cwd, name }
  trace: null, // { trace_id, cwd }
  entries: [], // EntryMeta[]
  index: 0,
  cursor: "", // feed cursor (newest seen timestamp)
  seen: new Set(), // "trace_id:index" keys already surfaced
};

async function api(path) {
  const res = await fetch(path);
  if (!res.ok) throw new Error(`${path} -> ${res.status}`);
  return res.json();
}

function esc(text) {
  const div = document.createElement("div");
  div.textContent = text == null ? "" : String(text);
  return div.innerHTML;
}

function relTime(iso) {
  const then = Date.parse(iso);
  if (isNaN(then)) return iso || "";
  const secs = Math.max(0, (Date.now() - then) / 1000);
  if (secs < 60) return "just now";
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
  return `${Math.floor(secs / 86400)}d ago`;
}

function relPath(path, cwd) {
  if (cwd && path.startsWith(cwd + "/")) return path.slice(cwd.length + 1);
  return path;
}

// ---- projects ---------------------------------------------------------

async function showProjects() {
  state.where = "projects";
  state.project = null;
  state.trace = null;
  setHeader("deltoids", "traces across your projects", "");
  backEl.hidden = true;
  view.scrollTop = 0;

  let projects = [];
  try {
    projects = await api("/api/projects");
  } catch (e) {
    view.innerHTML = `<div class="empty">Could not load projects.</div>`;
    return;
  }
  if (!projects.length) {
    view.innerHTML = `<div class="empty">No traces yet.<br />Make an edit with the deltoids tools.</div>`;
    return;
  }
  const cards = projects
    .map(
      (p) => `
      <button class="card" data-project="${esc(p.id)}">
        <div class="card-title">${esc(p.name)}</div>
        <div class="card-sub">${esc(p.cwd)}</div>
        <div class="card-meta">${p.trace_count} sessions · ${p.entry_count} edits · ${esc(
          relTime(p.last_timestamp),
        )}</div>
      </button>`,
    )
    .join("");
  view.innerHTML = `<div class="list">${cards}</div>`;
  view.querySelectorAll("[data-project]").forEach((el) => {
    el.addEventListener("click", () => {
      const project = projects.find((p) => p.id === el.dataset.project);
      showTraces(project);
    });
  });
}

// ---- traces -----------------------------------------------------------

async function showTraces(project) {
  state.where = "traces";
  state.project = project;
  setHeader(project.name, project.cwd, "");
  backEl.hidden = false;
  view.scrollTop = 0;

  let traces = [];
  try {
    traces = await api(`/api/projects/${project.id}/traces`);
  } catch (e) {
    view.innerHTML = `<div class="empty">Could not load sessions.</div>`;
    return;
  }
  if (!traces.length) {
    view.innerHTML = `<div class="empty">No sessions.</div>`;
    return;
  }
  const cards = traces
    .map(
      (t) => `
      <button class="card" data-trace="${esc(t.trace_id)}">
        <div class="card-title">${esc(t.last_reason || "(no summary)")}</div>
        <div class="card-sub">${esc(relPath(t.last_path, t.cwd))}</div>
        <div class="card-meta">${t.entry_count} edits · ${esc(relTime(t.last_timestamp))}</div>
      </button>`,
    )
    .join("");
  view.innerHTML = `<div class="list">${cards}</div>`;
  view.querySelectorAll("[data-trace]").forEach((el) => {
    el.addEventListener("click", () => {
      const trace = traces.find((t) => t.trace_id === el.dataset.trace);
      openTrace(trace.trace_id, trace.cwd, 0);
    });
  });
}

// ---- reviewer ---------------------------------------------------------

async function openTrace(traceId, cwd, index) {
  let data;
  try {
    data = await api(`/api/traces/${traceId}/entries`);
  } catch (e) {
    return;
  }
  state.where = "reviewer";
  state.trace = { trace_id: traceId, cwd: data.cwd || cwd };
  state.entries = data.entries;
  state.index = index === "last" ? data.entries.length - 1 : index;
  if (state.index < 0) state.index = 0;
  backEl.hidden = false;
  await renderEntry();
}

async function renderEntry() {
  const meta = state.entries[state.index];
  if (!meta) return;
  setHeader(
    meta.reason || "(no summary)",
    relPath(meta.path, state.trace.cwd),
    `${state.index + 1}/${state.entries.length}`,
  );

  let detail;
  try {
    detail = await api(`/api/traces/${state.trace.trace_id}/entries/${state.index}`);
  } catch (e) {
    view.innerHTML = `<div class="empty">Could not load edit.</div>`;
    return;
  }

  const errorBlock = detail.error
    ? `<div class="entry-error">${esc(detail.error)}</div>`
    : "";
  view.innerHTML = `
    <div class="reviewer">
      <div class="entry-head">
        <div class="entry-reason">${esc(detail.reason || "(no summary)")}</div>
        <div class="entry-path">${esc(relPath(detail.path, state.trace.cwd))}</div>
      </div>
      ${errorBlock}
      <div class="diff">${detail.html || ""}</div>
      <div class="navhint">swipe right → next · swipe left → back</div>
    </div>`;

  centerFirstChange();
  prefetch(state.index + 1);
  prefetch(state.index - 1);
}

function centerFirstChange() {
  const target = view.querySelector("[data-first-change]");
  if (!target) {
    view.scrollTop = 0;
    return;
  }
  const viewH = view.clientHeight;
  const top = target.offsetTop - viewH / 2 + target.offsetHeight / 2;
  view.scrollTop = Math.max(0, top);
}

const prefetched = new Set();
function prefetch(index) {
  if (index < 0 || index >= state.entries.length) return;
  const key = `${state.trace.trace_id}:${index}`;
  if (prefetched.has(key)) return;
  prefetched.add(key);
  fetch(`/api/traces/${state.trace.trace_id}/entries/${index}`).catch(() => {});
}

function go(delta) {
  if (state.where !== "reviewer") return;
  const next = state.index + delta;
  if (next < 0 || next >= state.entries.length) return;
  state.index = next;
  renderEntry();
}

// ---- header + navigation ---------------------------------------------

function setHeader(title, subtitle, counter) {
  titleEl.textContent = title;
  subtitleEl.textContent = subtitle;
  counterEl.textContent = counter;
}

backEl.addEventListener("click", () => {
  if (state.where === "reviewer") {
    if (state.project) showTraces(state.project);
    else showProjects();
  } else if (state.where === "traces") {
    showProjects();
  }
});

document.addEventListener("keydown", (e) => {
  if (state.where !== "reviewer") return;
  if (e.key === "ArrowRight") go(1);
  else if (e.key === "ArrowLeft") go(-1);
});

// Horizontal swipe on the reviewer. Vertical scrolling of long diffs must
// keep working, so only strong, mostly-horizontal gestures count.
let touchX = 0;
let touchY = 0;
let tracking = false;
view.addEventListener(
  "touchstart",
  (e) => {
    if (state.where !== "reviewer" || e.touches.length !== 1) return;
    touchX = e.touches[0].clientX;
    touchY = e.touches[0].clientY;
    tracking = true;
  },
  { passive: true },
);
view.addEventListener(
  "touchend",
  (e) => {
    if (!tracking) return;
    tracking = false;
    const t = e.changedTouches[0];
    const dx = t.clientX - touchX;
    const dy = t.clientY - touchY;
    if (Math.abs(dx) > 60 && Math.abs(dx) > Math.abs(dy) * 1.5) {
      go(dx > 0 ? 1 : -1); // swipe right → next, left → back
    }
  },
  { passive: true },
);

// ---- live feed --------------------------------------------------------

async function primeFeed() {
  try {
    const feed = await api("/api/feed");
    feed.entries.forEach((e) => state.seen.add(`${e.trace_id}:${e.index}`));
    state.cursor = feed.cursor || "";
  } catch (e) {
    /* ignore */
  }
}

async function pollFeed() {
  try {
    const q = state.cursor ? `?since=${encodeURIComponent(state.cursor)}` : "";
    const feed = await api(`/api/feed${q}`);
    const fresh = feed.entries.filter(
      (e) => !state.seen.has(`${e.trace_id}:${e.index}`),
    );
    feed.entries.forEach((e) => state.seen.add(`${e.trace_id}:${e.index}`));
    if (feed.cursor) state.cursor = feed.cursor;
    if (!fresh.length) return;

    const newest = fresh[0];
    // If we are already reviewing this trace, extend it in place.
    if (state.where === "reviewer" && state.trace.trace_id === newest.trace_id) {
      const data = await api(`/api/traces/${newest.trace_id}/entries`);
      state.entries = data.entries;
      counterEl.textContent = `${state.index + 1}/${state.entries.length}`;
    }
    showToast(newest);
  } catch (e) {
    /* ignore */
  }
}

let toastTimer = null;
function showToast(entry) {
  toastEl.textContent = `New edit · ${entry.project_name}`;
  toastEl.hidden = false;
  toastEl.onclick = () => {
    toastEl.hidden = true;
    openTrace(entry.trace_id, entry.cwd, "last");
  };
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    toastEl.hidden = true;
  }, 8000);
}

// ---- boot -------------------------------------------------------------

primeFeed().then(showProjects);
setInterval(pollFeed, 4000);
