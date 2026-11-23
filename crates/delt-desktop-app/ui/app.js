// DeltOS desktop window. No framework and no build step: the page calls the
// commands in src/main.rs and shows what comes back. Pack pages load in the
// sandboxed #page frame from deltpack: and can reach nothing here.
"use strict";

const T = window.__TAURI__;
const call = (cmd, args) => T.core.invoke(cmd, args);
const $ = (s) => document.querySelector(s);
const el = (tag, props = {}, ...kids) => {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === "class") n.className = v;
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else if (v !== undefined && v !== null && v !== false) n.setAttribute(k, v === true ? "" : v);
  }
  for (const k of kids.flat()) if (k !== null && k !== undefined) n.append(k.nodeType ? k : String(k));
  return n;
};

const state = {
  packs: [],
  byId: new Map(),
  current: null, // { pack, path, title }
  view: "library",
  query: "",
  next: null,
};

// ---------- small helpers ----------

function toast(msg, err = false) {
  const t = $("#toast");
  t.textContent = msg;
  t.className = "toast" + (err ? " err" : "");
  t.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => (t.hidden = true), err ? 8000 : 4000);
}

async function attempt(fn, okMsg) {
  try {
    const r = await fn();
    if (okMsg) toast(typeof okMsg === "function" ? okMsg(r) : okMsg);
    return r;
  } catch (e) {
    toast(String(e), true);
    return undefined;
  }
}

function confirmBox(title, text, okLabel, danger = false) {
  const d = $("#confirm");
  $("#confirm-title").textContent = title;
  const body = $("#confirm-text");
  body.replaceChildren(...(Array.isArray(text) ? text : [el("p", {}, text)]));
  const ok = $("#confirm-ok");
  ok.textContent = okLabel;
  ok.className = danger ? "danger" : "primary";
  d.returnValue = "";
  d.showModal();
  return new Promise((res) => d.addEventListener("close", () => res(d.returnValue === "ok"), { once: true }));
}

const bytes = (n) => {
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1000 && i < u.length - 1) { n /= 1000; i++; }
  return `${n.toFixed(n < 10 && i ? 1 : 0)} ${u[i]}`;
};
const when = (ms) => new Date(ms).toLocaleString();
const timeOf = (ms) => new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
// Chunking: people read "English", not "en-US".
const langNames = (() => { try { return new Intl.DisplayNames([navigator.language || "en"], { type: "language" }); } catch { return null; } })();
// Fallback for web views without Intl.DisplayNames (seen on WebKitGTK).
const LANGS = { ar: "Arabic", be: "Belarusian", bn: "Bengali", de: "German", en: "English", es: "Spanish", fa: "Persian", fr: "French", hi: "Hindi", id: "Indonesian", it: "Italian", ja: "Japanese", ko: "Korean", ms: "Malay", nl: "Dutch", pl: "Polish", pt: "Portuguese", ru: "Russian", sw: "Swahili", ta: "Tamil", th: "Thai", tl: "Tagalog", tr: "Turkish", uk: "Ukrainian", ur: "Urdu", vi: "Vietnamese", zh: "Chinese" };
const langName = (tag) => {
  try { const n = langNames?.of(tag); if (n && n.toLowerCase() !== tag.toLowerCase()) return n; } catch {}
  return LANGS[tag.split("-")[0].toLowerCase()] || tag;
};
function dayLabel(ms) {
  const d = new Date(ms), now = new Date();
  const start = (x) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((start(now) - start(d)) / 86400000);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  return d.toLocaleDateString([], { weekday: days < 7 ? "long" : undefined, day: "numeric", month: "long", year: d.getFullYear() === now.getFullYear() ? undefined : "numeric" });
}
// Fitts: a whole row or tile is the target, not only its title. The link
// stretches over its card (CSS .stretch); other buttons sit above it.
const stretchLink = (text, onOpen, label) => el("a", { href: "#", class: "stretch", "aria-label": label, onclick: (e) => { e.preventDefault(); onOpen(); } }, text);
const packName = (id) => state.byId.get(id)?.name || "A pack that is not in the library";

// ---------- views ----------

function show(view) {
  state.view = view;
  for (const s of document.querySelectorAll("main > .view")) s.hidden = s.id !== `view-${view}`;
  for (const b of document.querySelectorAll(".tabs button")) {
    if (b.dataset.view === view) b.setAttribute("aria-current", "page");
    else b.removeAttribute("aria-current");
  }
  const render = { library: renderLibrary, bookmarks: renderBookmarks, notes: renderNotes, history: renderHistory, settings: renderSettings }[view];
  if (render) render();
}

for (const b of document.querySelectorAll(".tabs button")) b.addEventListener("click", () => show(b.dataset.view));

// ---------- theme ----------

async function applyTheme(s) {
  const root = document.documentElement;
  if (s.theme === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", s.theme);
  if (s.colorblind) root.setAttribute("data-colorblind", "on");
  else root.removeAttribute("data-colorblind");
}

async function renderSettings() {
  const s = await call("settings");
  for (const r of document.querySelectorAll('input[name="theme"]')) r.checked = r.value === s.theme;
  $("#colorblind").checked = s.colorblind;
  $("#lib-dir").textContent = await call("library_dir");
}

async function saveAppearance() {
  const theme = document.querySelector('input[name="theme"]:checked')?.value || "dark";
  const s = await attempt(() => call("save_appearance", { theme, colorblind: $("#colorblind").checked }));
  if (s) applyTheme(s);
}
for (const r of document.querySelectorAll('input[name="theme"]')) r.addEventListener("change", saveAppearance);
$("#colorblind").addEventListener("change", saveAppearance);

// ---------- library ----------

async function loadPacks() {
  state.packs = await call("packs");
  state.byId = new Map(state.packs.map((p) => [p.archive_uuid, p]));
}

async function renderLibrary() {
  await attempt(() => call("refresh"));
  await loadPacks();
  renderContinue();
  const box = $("#packs");
  if (!state.packs.length) {
    box.replaceChildren(el("div", { class: "empty" },
      el("p", {}, el("b", {}, "Your library is empty.")),
      el("p", {}, "Add a .wax pack from this computer or a USB drive to start reading."),
      el("button", { class: "primary", onclick: () => $("#add-pack").click() }, "Add a pack…")));
    return;
  }
  // Von Restorff: only a pack that has something to say is featured.
  box.replaceChildren(...state.packs.map((p, i) => tile(p, i === 0 && state.packs.length > 2 && !!p.description)));
}

// Zeigarnik / serial position: the unfinished thing, first.
async function renderContinue() {
  const list = await call("history", { limit: 30 }).catch(() => []);
  const seen = new Set();
  const recent = [];
  for (const v of list) {
    if (!state.byId.has(v.pack) || seen.has(v.pack)) continue;
    seen.add(v.pack);
    recent.push(v);
    if (recent.length === 3) break;
  }
  $("#continue").hidden = !recent.length;
  $("#continue-list").replaceChildren(...recent.map((v) => el("li", { class: "card" },
    el("span", { class: "pk" }, packName(v.pack)),
    stretchLink(v.title, () => openPage(v.pack, v.path), `Continue reading ${v.title}, in ${packName(v.pack)}`),
    el("span", { class: "when" }, `${dayLabel(v.visited_at)}, ${timeOf(v.visited_at)}`))));
}

function tile(p, big) {
  const name = p.title || p.name || p.file;
  const facts = [p.languages.map(langName).join(", "), bytes(p.file_size), p.version && `Edition ${p.version}`].filter(Boolean);
  const search = p.searchable
    ? el("span", { class: "state ok" }, "Searchable")
    : el("span", { class: "state warn" }, "No search index");
  // Hick: one obvious action (open, by clicking anywhere on the tile); the
  // rarer ones wait behind "More".
  const more = el("details", { class: "more" },
    el("summary", { "aria-label": `More for ${name}`, title: "More" }, "⋯"),
    el("div", { class: "menu", role: "group", "aria-label": `More for ${name}` },
      el("button", { onclick: () => updateFromFile(p) }, "Update from a file…"),
      p.can_undo_update ? el("button", { onclick: () => undoUpdate(p) }, "Undo last update") : null,
      el("button", { class: "danger", onclick: () => removePack(p) }, "Remove from library…")));
  return el("article", { class: "tile" + (big ? " big" : "") },
    el("h2", {}, stretchLink(name, () => openPage(p.archive_uuid, p.entry_point || ""), `Open ${name}`)),
    p.description ? el("p", { class: "desc" }, p.description) : el("span", { class: "grow" }),
    el("div", { class: "facts" }, ...facts.map((f) => el("span", {}, f))),
    el("div", { class: "foot" }, search, more));
}
document.addEventListener("click", (e) => {
  for (const d of document.querySelectorAll("details.more[open]")) if (!d.contains(e.target)) d.open = false;
});

$("#add-pack").addEventListener("click", async () => {
  const path = await call("pick_pack");
  if (!path) return;
  const copy = await confirmBox("Add this pack?",
    [el("p", { class: "mono" }, path),
     el("p", {}, "DeltOS copies the pack into your library folder and leaves the original where it is. A large pack can take a few minutes to copy.")],
    "Copy into library");
  if (!copy) return;
  toast("Adding the pack…");
  const p = await attempt(() => call("import_pack", { path, mode: "copy" }), (p) => `Added ${p.name}.`);
  if (p) renderLibrary();
});

$("#change-dir").addEventListener("click", async () => {
  const path = await call("pick_folder");
  if (!path) return;
  const ok = await attempt(() => call("set_library_dir", { path }), "Library folder changed.");
  if (ok !== undefined) { await loadPacks(); renderSettings(); }
});

async function removePack(p) {
  const ok = await confirmBox(`Remove ${p.name}?`,
    "This deletes the pack file from your library folder. Your bookmarks and notes about it are kept.",
    "Delete pack", true);
  if (!ok) return;
  if (await attempt(() => call("remove_pack", { uuid: p.archive_uuid }), `Removed ${p.name}.`) !== undefined) renderLibrary();
}

function describe(plan) {
  const from = plan.installed_version || "the installed edition";
  const to = plan.candidate_version || "a newer edition";
  const how = plan.kind.kind === "append"
    ? `DeltOS adds the ${bytes(plan.kind.bytes)} that changed to your copy. You can undo it afterwards.`
    : `This edition was rebuilt, so DeltOS replaces the whole file (${bytes(plan.kind.bytes)}). This cannot be undone.`;
  const sig = plan.signature_sidecar
    ? "It comes with a signature file. DeltOS keeps it, but does not check it yet."
    : "It has no signature file.";
  return [el("p", {}, `${plan.name}: ${from} → ${to}.`), el("p", {}, how), el("p", { class: "meta" }, sig)];
}

async function applyPlan(plan) {
  const ok = await confirmBox("Apply this update?", describe(plan), "Update");
  if (!ok) return;
  toast("Updating…");
  const r = await attempt(() => call("apply_update", { path: plan.candidate }),
    (r) => `${plan.name} is now ${r.version || "up to date"}.`);
  if (r) { $("#updates-found").replaceChildren(); renderLibrary(); }
}

async function updateFromFile(p) {
  const path = await call("pick_pack");
  if (!path) return;
  const plan = await attempt(() => call("check_update", { path }));
  if (plan) applyPlan(plan);
}

async function undoUpdate(p) {
  const ok = await confirmBox(`Undo the last update to ${p.name}?`,
    "The pack goes back exactly to the edition it was before that update.", "Undo update");
  if (!ok) return;
  if (await attempt(() => call("undo_update", { uuid: p.archive_uuid }), (r) => `${p.name} is back to ${r.version || "its earlier edition"}.`)) renderLibrary();
}

$("#find-updates").addEventListener("click", async () => {
  const dir = await call("pick_folder");
  if (!dir) return;
  const plans = await attempt(() => call("find_updates", { dir }));
  if (!plans) return;
  const box = $("#updates-found");
  if (!plans.length) { box.replaceChildren(); toast("No newer editions of your packs are in that folder."); return; }
  box.replaceChildren(el("div", { class: "panel update" },
    el("div", { class: "rule", "aria-hidden": "true" }, el("i"), el("i"), el("i")),
    el("h2", {}, `${plans.length} update${plans.length > 1 ? "s" : ""} found`),
    ...plans.map((pl) => el("div", { class: "row" },
      el("div", { class: "what" }, describe(pl)[0].textContent),
      el("button", { class: "primary", onclick: () => applyPlan(pl) }, "Update…")))));
});

// ---------- reading ----------

function pageUrl(pack, path) {
  const p = state.byId.get(pack);
  if (!p) return null;
  const enc = path.split("/").map(encodeURIComponent).join("/");
  return `${p.origin}/${enc}`;
}

async function openPage(pack, path) {
  if (!state.byId.size) await loadPacks();
  const url = pageUrl(pack, path);
  if (!url) { toast("That pack is not in your library any more.", true); return; }
  $("#tab-reader").hidden = false;
  show("reader");
  loading(true);
  $("#page").src = url;
}

// Doherty threshold: answer at once, even if the page takes a moment.
let loadTimer = null;
function loading(on) {
  clearTimeout(loadTimer);
  if (on) loadTimer = setTimeout(() => ($("#loading").hidden = false), 150);
  else $("#loading").hidden = true;
}
$("#page").addEventListener("load", () => loading(false));

// the pack zone tells us each page the frame shows
T.event.listen("delt-navigated", (ev) => {
  state.current = ev.payload;
  const p = state.byId.get(ev.payload.pack);
  $("#crumbs").replaceChildren(p ? p.name : "", " › ", el("b", {}, ev.payload.title));
  document.title = `${ev.payload.title} — DeltOS`;
  refreshPageChrome();
});

async function refreshPageChrome() {
  const c = state.current;
  if (!c) return;
  const id = await call("bookmark_of", { pack: c.pack, path: c.path });
  const b = $("#bookmark-toggle");
  b.setAttribute("aria-pressed", id ? "true" : "false");
  b.textContent = id ? "Bookmarked" : "Bookmark";
  b.dataset.id = id || "";
  if (!$("#page-notes").hidden) renderPageNotes();
}

$("#back").addEventListener("click", () => history.back());
$("#forward").addEventListener("click", () => history.forward());
$("#home").addEventListener("click", () => {
  const c = state.current;
  const p = c && state.byId.get(c.pack);
  if (p) openPage(p.archive_uuid, p.entry_point || "");
});

$("#bookmark-toggle").addEventListener("click", async () => {
  const c = state.current;
  if (!c) return;
  const b = $("#bookmark-toggle");
  if (b.dataset.id) await attempt(() => call("remove_bookmark", { id: Number(b.dataset.id) }), "Bookmark removed.");
  else await attempt(() => call("add_bookmark", { pack: c.pack, path: c.path, title: c.title }), "Bookmarked.");
  refreshPageChrome();
});

$("#notes-toggle").addEventListener("click", () => {
  const side = $("#page-notes");
  side.hidden = !side.hidden;
  $("#notes-toggle").setAttribute("aria-expanded", String(!side.hidden));
  if (!side.hidden) { renderPageNotes(); $("#note-body").focus(); }
});

$("#note-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const c = state.current;
  if (!c) return;
  const n = await attempt(() => call("add_note", {
    pack: c.pack, path: c.path, pageTitle: c.title,
    quote: $("#note-quote").value, body: $("#note-body").value,
  }), "Note saved.");
  if (n) { $("#note-quote").value = ""; $("#note-body").value = ""; renderPageNotes(); }
});

function noteItem(n, withPage, rerender) {
  const body = el("p", { class: "body" }, n.body);
  return el("li", {},
    withPage ? el("p", {}, el("a", { href: "#", onclick: (e) => { e.preventDefault(); openPage(n.pack, n.path); } }, n.page_title),
      " ", el("span", { class: "pk" }, packName(n.pack))) : null,
    n.quote ? el("blockquote", {}, n.quote) : null,
    body,
    el("div", { class: "row" },
      el("span", { class: "when" }, when(n.updated_at)),
      el("button", { onclick: async () => {
        const ta = el("textarea", { rows: 4 });
        ta.value = n.body;
        const ok = await confirmBox("Edit note", [ta], "Save");
        if (ok) { await attempt(() => call("update_note", { id: n.id, body: ta.value }), "Note saved."); rerender(); }
      } }, "Edit"),
      el("button", { class: "danger", onclick: async () => {
        if (await confirmBox("Delete this note?", "It cannot be brought back.", "Delete", true)) {
          await attempt(() => call("remove_note", { id: n.id }), "Note deleted.");
          rerender();
        }
      } }, "Delete")));
}

async function renderPageNotes() {
  const c = state.current;
  if (!c) return;
  const list = await call("notes", { pack: c.pack, path: c.path });
  $("#page-note-list").replaceChildren(...list.map((n) => noteItem(n, false, renderPageNotes)));
}

// ---------- search ----------

const qInput = $("#q");
const sug = $("#suggest");
let sugTimer = null;
let sugSel = -1;

qInput.addEventListener("input", () => {
  clearTimeout(sugTimer);
  const q = qInput.value;
  if (!q.trim()) { closeSuggest(); return; }
  sugTimer = setTimeout(async () => {
    const r = await call("api_get", { path: "/suggest", query: `q=${encodeURIComponent(q)}&limit=8` });
    if (qInput.value !== q || r.status !== 200) return;
    const items = r.body.items || [];
    sugSel = -1;
    sug.replaceChildren(...items.map((it, i) => el("li", {
      role: "option", id: `sug-${i}`,
      onmousedown: (e) => { e.preventDefault(); closeSuggest(); openPage(it.pack, it.path); },
    }, el("span", {}, it.title), el("span", { class: "pk" }, packName(it.pack)))));
    sug.hidden = !items.length;
    qInput.setAttribute("aria-expanded", String(!!items.length));
    sug._items = items;
  }, 120);
});

qInput.addEventListener("keydown", (e) => {
  if (sug.hidden) return;
  const n = sug.children.length;
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    if (e.key === "ArrowDown") sugSel = (sugSel + 1) % n;
    else sugSel = sugSel <= 0 ? n - 1 : sugSel - 1;
    [...sug.children].forEach((li, i) => li.setAttribute("aria-selected", String(i === sugSel)));
    qInput.setAttribute("aria-activedescendant", `sug-${sugSel}`);
  } else if (e.key === "Enter" && sugSel >= 0) {
    e.preventDefault();
    const it = sug._items[sugSel];
    closeSuggest();
    openPage(it.pack, it.path);
  } else if (e.key === "Escape") {
    closeSuggest();
  }
});
qInput.addEventListener("blur", () => setTimeout(closeSuggest, 100));

function closeSuggest() {
  sug.hidden = true;
  sugSel = -1;
  qInput.setAttribute("aria-expanded", "false");
  qInput.removeAttribute("aria-activedescendant");
}

$("#search-form").addEventListener("submit", (e) => {
  e.preventDefault();
  closeSuggest();
  const q = qInput.value.trim();
  if (q) runSearch(q, null);
});

const SKIP = {
  no_index: "has no search index",
  unknown_tokenizer: "was indexed in a way this version of DeltOS does not know",
  tokenizer_mismatch: "has an index that does not match its declared kind",
  query_too_short: "needs a longer search for its language",
  query_not_supported: "cannot answer this search",
};

async function runSearch(q, cursor) {
  if (!state.byId.size) await loadPacks();
  show("search");
  $("#search-q").textContent = `“${q}”`;
  if (!cursor) { $("#results").replaceChildren(); $("#search-skipped").textContent = ""; $("#more-results").hidden = true; }
  $("#search-status").textContent = "Searching…";
  const r = await call("search", { q, limit: 20, cursor });
  $("#search-status").textContent = "";
  if (r.status !== 200) { $("#results").replaceChildren(el("li", {}, r.body?.title || "Search failed.")); return; }
  const b = r.body;
  const items = b.items.map((it) => el("li", { class: "card" },
    el("span", { class: "pk" }, it.pack_name || packName(it.pack)),
    stretchLink(it.title || it.path, () => openPage(it.pack, it.path), `${it.title || it.path}, in ${it.pack_name || packName(it.pack)}`),
    it.snippet ? el("p", {}, it.snippet) : null));
  if (cursor) $("#results").append(...items);
  else $("#results").replaceChildren(...(items.length ? items : [el("li", { class: "nothing" }, "Nothing found. Try fewer words, or a different spelling.")]));
  $("#search-skipped").textContent = (b.skipped || [])
    .map((s) => `${s.name || "A pack"} ${SKIP[s.reason] || "was not searched"}.`).join(" ");
  state.query = q;
  state.next = b.next;
  $("#more-results").hidden = !b.next;
}
$("#more-results").addEventListener("click", () => state.next && runSearch(state.query, state.next));

// ---------- bookmarks, notes, history ----------

async function renderBookmarks() {
  await loadPacks();
  const list = await call("bookmarks");
  $("#bookmark-list").replaceChildren(...(list.length ? list.map((b) => el("li", { class: "card" },
    el("div", {},
      el("span", { class: "pk" }, packName(b.pack)),
      stretchLink(b.title, () => openPage(b.pack, b.path), `${b.title}, in ${packName(b.pack)}`)),
    el("button", { class: "lift", "aria-label": `Remove bookmark ${b.title}`, onclick: async () => { await attempt(() => call("remove_bookmark", { id: b.id }), "Bookmark removed."); renderBookmarks(); } }, "Remove")))
    : [el("li", { class: "nothing" }, "No bookmarks yet. Press Bookmark (or Ctrl D) while reading a page.")]));
}

async function renderNotes() {
  await loadPacks();
  const list = await call("notes", {});
  $("#all-notes").replaceChildren(...(list.length ? list.map((n) => noteItem(n, true, renderNotes))
    : [el("li", { class: "nothing" }, "No notes yet. Press Notes while reading a page to write one.")]));
}

$("#export-notes").addEventListener("click", () =>
  attempt(() => call("export_notes"), (p) => (p ? `Saved to ${p}.` : "Not saved.")));

async function renderHistory() {
  await loadPacks();
  const list = await call("history", { limit: 200 });
  // Common region: one group per day, newest first.
  const groups = [];
  for (const v of list) {
    const label = dayLabel(v.visited_at);
    if (!groups.length || groups[groups.length - 1].label !== label) groups.push({ label, items: [] });
    groups[groups.length - 1].items.push(v);
  }
  $("#history-list").replaceChildren(...(groups.length ? groups.map((g) => el("li", { class: "day" },
    el("h2", { class: "eyebrow" }, g.label),
    el("ol", { class: "rows" }, ...g.items.map((v) => el("li", { class: "card" },
      el("div", {},
        el("span", { class: "pk" }, packName(v.pack)),
        stretchLink(v.title, () => openPage(v.pack, v.path), `${v.title}, in ${packName(v.pack)}`)),
      el("span", { class: "when" }, timeOf(v.visited_at)))))))
    : [el("li", { class: "nothing" }, "Nothing read yet. Pages you open show up here.")]));
}

$("#clear-history").addEventListener("click", async () => {
  if (await confirmBox("Clear history?", "The list of pages you have read is deleted. Bookmarks and notes stay.", "Clear", true)) {
    await attempt(() => call("clear_history"), "History cleared.");
    renderHistory();
  }
});

// ---------- keyboard (Jakob's law: the shortcuts people know from browsers) ----------

const isMac = /Mac|iPhone|iPad/.test(navigator.platform || "");
$("#search-hint").textContent = isMac ? "⌘K" : "Ctrl K";
document.addEventListener("keydown", (e) => {
  const mod = isMac ? e.metaKey : e.ctrlKey;
  const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName || "");
  if ((mod && e.key.toLowerCase() === "k") || (e.key === "/" && !typing && !mod)) {
    e.preventDefault(); qInput.focus(); qInput.select();
  } else if (state.view === "reader" && e.altKey && !typing && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
    e.preventDefault(); e.key === "ArrowLeft" ? history.back() : history.forward();
  } else if (state.view === "reader" && mod && e.key.toLowerCase() === "d") {
    e.preventDefault(); $("#bookmark-toggle").click();
  }
});

// ---------- start ----------

(async function start() {
  $("#tokens").textContent = await call("theme_css");
  applyTheme(await call("settings"));
  show("library");
})();
