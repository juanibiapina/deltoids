import {
  WASI,
  OpenFile,
  File as WasiFile,
  ConsoleStdout,
} from "./vendor/browser_wasi_shim.js";
import { parsePrUrl, decodeBase64Utf8, looksBinary, badgeClass } from "./lib.js";

// ---------------------------------------------------------------------------
// wasm engine
// ---------------------------------------------------------------------------

// Lazily instantiated deltoids wasm module wrapped in a `renderFile` helper.
let enginePromise = null;

function loadEngine() {
  if (!enginePromise) enginePromise = instantiateEngine();
  return enginePromise;
}

async function instantiateEngine() {
  const wasi = new WASI([], [], [
    new OpenFile(new WasiFile([])),
    ConsoleStdout.lineBuffered((m) => console.log("[wasm]", m)),
    ConsoleStdout.lineBuffered((m) => console.warn("[wasm]", m)),
  ]);
  const importObject = { wasi_snapshot_preview1: wasi.wasiImport };

  let instance;
  try {
    const source = await WebAssembly.instantiateStreaming(
      fetch("./deltoids_wasm.wasm"),
      importObject,
    );
    instance = source.instance;
  } catch {
    // Fall back when the server does not send application/wasm.
    const bytes = await (await fetch("./deltoids_wasm.wasm")).arrayBuffer();
    const source = await WebAssembly.instantiate(bytes, importObject);
    instance = source.instance;
  }
  wasi.initialize(instance);

  const { memory, alloc, dealloc, render_file, render_from_patch } = instance.exports;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  function put(str) {
    const bytes = encoder.encode(str);
    const ptr = alloc(bytes.length) >>> 0;
    new Uint8Array(memory.buffer, ptr, bytes.length).set(bytes);
    return [ptr, bytes.length];
  }

  // Read a packed `ptr << 32 | len` result into a string and free it.
  function takeResult(packed) {
    const ptr = Number(packed >> 32n) >>> 0;
    const len = Number(packed & 0xffffffffn);
    const html = decoder.decode(new Uint8Array(memory.buffer, ptr, len).slice());
    dealloc(ptr, len);
    return html;
  }

  // Compute the deltoids diff HTML body from full before/after content.
  function renderFile(before, after, path) {
    const args = [put(before), put(after), put(path)];
    const html = takeResult(render_file(...args.flat()));
    for (const [p, l] of args) dealloc(p, l);
    return html;
  }

  // Compute the diff HTML from after content plus a unified patch, letting the
  // engine reconstruct the before side (one fewer GitHub request).
  function renderFromPatch(after, patch, path) {
    const args = [put(after), put(patch), put(path)];
    const html = takeResult(render_from_patch(...args.flat()));
    for (const [p, l] of args) dealloc(p, l);
    return html;
  }

  return { renderFile, renderFromPatch };
}

// ---------------------------------------------------------------------------
// GitHub API client
// ---------------------------------------------------------------------------

const TOKEN_KEY = "deltoids.gh.token";

function token() {
  return localStorage.getItem(TOKEN_KEY) || "";
}

function ghHeaders() {
  const headers = { Accept: "application/vnd.github+json" };
  const t = token();
  if (t) headers.Authorization = `Bearer ${t}`;
  return headers;
}

async function gh(url) {
  const res = await fetch(url, { headers: ghHeaders() });
  const remaining = res.headers.get("x-ratelimit-remaining");
  if (!res.ok) {
    const body = await res.text();
    let message = `${res.status} ${res.statusText}`;
    if (res.status === 403 && remaining === "0") {
      message = token()
        ? "GitHub rate limit reached for this token."
        : "GitHub rate limit reached (60/hr). Add a token with 🔑 for 5000/hr.";
    } else if (res.status === 404) {
      message = "Not found. Private repo? Add a token with 🔑.";
    }
    const err = new Error(message);
    err.detail = body;
    throw err;
  }
  return { data: await res.json(), remaining };
}

async function fetchPr({ owner, repo, number }) {
  const { data } = await gh(
    `https://api.github.com/repos/${owner}/${repo}/pulls/${number}`,
  );
  return data;
}

async function fetchFiles({ owner, repo, number }) {
  const files = [];
  for (let page = 1; page <= 30; page++) {
    const { data } = await gh(
      `https://api.github.com/repos/${owner}/${repo}/pulls/${number}/files?per_page=100&page=${page}`,
    );
    files.push(...data);
    if (data.length < 100) break;
  }
  return files;
}

// Fetch a file's UTF-8 content at a ref, or "" when it does not exist there.
async function fetchContent({ owner, repo }, path, ref) {
  const url = `https://api.github.com/repos/${owner}/${repo}/contents/${encodeURIComponent(
    path,
  ).replace(/%2F/g, "/")}?ref=${encodeURIComponent(ref)}`;
  const res = await fetch(url, { headers: ghHeaders() });
  if (res.status === 404) return "";
  if (!res.ok) throw new Error(`content ${res.status} for ${path}`);
  const data = await res.json();
  if (data.encoding !== "base64" || typeof data.content !== "string") {
    throw new Error(`unexpected content encoding for ${path}`);
  }
  return decodeBase64Utf8(data.content);
}

// Render one changed file to HTML, or null when it is binary.
//
// For modified/renamed files that carry a `patch`, fetch only the head-side
// content and let the engine reconstruct the before side (one request instead
// of two). Otherwise resolve both sides.
async function renderOneFile(engine, repoRef, file, baseSha, headSha) {
  const canPatch =
    (file.status === "modified" || file.status === "renamed") &&
    typeof file.patch === "string" &&
    file.patch.length > 0;

  if (canPatch) {
    const after = await fetchContent(repoRef, file.filename, headSha);
    if (looksBinary(after)) return null;
    return engine.renderFromPatch(after, file.patch, file.filename);
  }

  const { before, after, path } = await resolveSides(repoRef, file, baseSha, headSha);
  if (looksBinary(before) || looksBinary(after)) return null;
  return engine.renderFile(before, after, path);
}

// Resolve before/after content for a changed file based on its status.
async function resolveSides(repoRef, file, baseSha, headSha) {
  const newPath = file.filename;
  const oldPath = file.previous_filename || file.filename;
  switch (file.status) {
    case "added":
      return { before: "", after: await fetchContent(repoRef, newPath, headSha), path: newPath };
    case "removed":
      return { before: await fetchContent(repoRef, oldPath, baseSha), after: "", path: oldPath };
    default: {
      const [before, after] = await Promise.all([
        fetchContent(repoRef, oldPath, baseSha),
        fetchContent(repoRef, newPath, headSha),
      ]);
      return { before, after, path: newPath };
    }
  }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

const statusEl = document.getElementById("status");
const appEl = document.getElementById("app");
const formEl = document.getElementById("pr-form");
const inputEl = document.getElementById("pr-url");

function setStatus(text, isError = false) {
  statusEl.textContent = text;
  statusEl.classList.toggle("error", isError);
}

async function review(input) {
  const ref = parsePrUrl(input);
  if (!ref) {
    setStatus("Enter a GitHub PR URL like https://github.com/owner/repo/pull/123", true);
    return;
  }

  appEl.innerHTML = "";
  setStatus("Loading engine and PR…");

  try {
    const [engine, pr, files] = await Promise.all([
      loadEngine(),
      fetchPr(ref),
      fetchFiles(ref),
    ]);

    const layout = document.createElement("div");
    layout.className = "layout";
    const sidebar = document.createElement("nav");
    sidebar.className = "sidebar";
    const column = document.createElement("div");
    column.className = "column";
    layout.append(sidebar, column);
    appEl.appendChild(layout);

    const meta = document.createElement("div");
    meta.className = "pr-meta";
    meta.innerHTML = `<h1></h1><div class="sub"></div>`;
    meta.querySelector("h1").textContent = `#${pr.number} · ${pr.title}`;
    const capped = files.length >= 3000 ? " (first 3000)" : "";
    meta.querySelector(".sub").textContent =
      `${ref.owner}/${ref.repo} · ${files.length} files${capped} · +${pr.additions} −${pr.deletions}`;
    column.appendChild(meta);

    const baseSha = pr.base.sha;
    const headSha = pr.head.sha;
    let loaded = 0;

    // Load each file's content and render it, at most once.
    const cards = new Map();
    async function loadCard(card) {
      if (card.loaded) return;
      card.loaded = true;
      try {
        const html = await renderOneFile(engine, ref, card.file, baseSha, headSha);
        card.body.innerHTML =
          html === null
            ? `<div class="notice">Binary file not shown.</div>`
            : html || `<div class="notice">No textual changes.</div>`;
      } catch (err) {
        card.body.innerHTML = `<div class="notice">Could not load: ${escapeHtml(
          err.message,
        )}</div>`;
      }
      loaded++;
      setStatus(`Loaded ${loaded}/${files.length} files.`);
    }

    // Render lazily: fetch + render a file only as it nears the viewport, so a
    // large PR does not fire thousands of GitHub requests up front.
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          const card = cards.get(entry.target);
          observer.unobserve(entry.target);
          if (card) loadCard(card);
        }
      },
      { rootMargin: "600px 0px" },
    );

    files.forEach((file, i) => {
      const card = renderFileShell(file);
      card.el.id = `file-${i}`;
      card.file = file;
      card.loaded = false;
      column.appendChild(card.el);
      cards.set(card.el, card);
      observer.observe(card.el);

      const link = document.createElement("a");
      link.href = `#file-${i}`;
      link.className = `side-item ${badgeClass(file.status)}`;
      link.textContent = file.filename;
      link.title = file.filename;
      sidebar.appendChild(link);
    });

    setStatus(`${files.length} files. Scroll to load.`);
  } catch (err) {
    setStatus(err.message, true);
    if (err.detail) console.error(err.detail);
  }
}

function renderFileShell(file) {
  const el = document.createElement("section");
  el.className = "file";
  const head = document.createElement("div");
  head.className = "file-head";
  const badge = badgeClass(file.status);
  head.innerHTML = `<span class="badge ${badge}">${file.status}</span><span class="path"></span>`;
  const label =
    file.status === "renamed"
      ? `${file.previous_filename} → ${file.filename}`
      : file.filename;
  head.querySelector(".path").textContent = label;
  const body = document.createElement("div");
  body.className = "diff";
  body.innerHTML = `<div class="notice">Loading…</div>`;
  el.appendChild(head);
  el.appendChild(body);
  return { el, body };
}

function escapeHtml(s) {
  return s.replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

// Token prompt.
document.getElementById("token-btn").addEventListener("click", () => {
  const current = token();
  const next = prompt(
    "GitHub personal access token (read-only, stored in this browser). Leave blank to clear.",
    current,
  );
  if (next === null) return;
  if (next.trim()) localStorage.setItem(TOKEN_KEY, next.trim());
  else localStorage.removeItem(TOKEN_KEY);
  setStatus(next.trim() ? "Token saved." : "Token cleared.");
});

formEl.addEventListener("submit", (e) => {
  e.preventDefault();
  const value = inputEl.value;
  const params = new URLSearchParams(location.search);
  params.set("pr", value);
  history.replaceState(null, "", `?${params.toString()}`);
  review(value);
});

// Deep link: ?pr=<url or owner/repo/number>
const initial = new URLSearchParams(location.search).get("pr");
if (initial) {
  inputEl.value = initial;
  review(initial);
} else {
  setStatus("Paste a GitHub PR URL and press Review.");
}
