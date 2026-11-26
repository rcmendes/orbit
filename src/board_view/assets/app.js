"use strict";
// Orbit Board view (ADR 0023). The server owns every rule: each Issue in the
// snapshot carries its column, drop targets, refusal reasons, menu entries and
// confirmation copy. This page only draws them and posts what the viewer picks.

// ---------- DOM helper: text nodes only, never parsed markup (Issue text is untrusted) ----------
// The HTML parser knows the SVG namespace, so the page carries no URL literal at all.
const SVG_NS = new DOMParser().parseFromString("<svg></svg>", "text/html").querySelector("svg").namespaceURI;
function h(tag, attrs, ...kids) {
  const el = tag.startsWith("svg:") ? document.createElementNS(SVG_NS, tag.slice(4)) : document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue;
    if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (k === "style") for (const [p, val] of Object.entries(v)) el.style.setProperty(p, val);
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids.flat(Infinity)) if (kid != null && kid !== false) el.append(kid.nodeType ? kid : String(kid));
  return el;
}
const $ = (id) => document.getElementById(id);

// ---------- token: from the fragment, else this tab's sessionStorage; only ever sent as a header ----------
const TOKEN_KEY = "orbit.token";
const NO_TOKEN = "This page has no valid token. Open the link that orbit board view printed.";
let TOKEN = (/(?:^#|&)token=([0-9a-f]{64})(?:&|$)/.exec(location.hash) || [])[1] || "";
try {
  if (TOKEN) sessionStorage.setItem(TOKEN_KEY, TOKEN);
  else TOKEN = sessionStorage.getItem(TOKEN_KEY) || "";
} catch {} // no storage: a reload shows the no-token banner
if (location.hash) history.replaceState(null, "", location.pathname + location.search);
// A 401 means the server restarted with a new token: drop ours and stop polling.
function forgetToken() {
  TOKEN = "";
  try { sessionStorage.removeItem(TOKEN_KEY); } catch {}
  S.offlineSince = Date.now();
  setBanner(NO_TOKEN);
  paintSync();
}

// ---------- constants ----------
const LAYOUTS = { columns: "Columns", queue: "Queue" };
const POLL_MS = 3000;
const LEASE_MS = 30 * 60e3;
const CW_PRESETS = [["24h", "24 hours"], ["3d", "3 days"], ["1w", "1 week"], ["30d", "1 month"], ["all", "All"]];
const CW_UNIT_MS = { h: 36e5, d: 864e5, w: 6048e5 };
const CW_KEY = "orbit.closedWindow";
const CAT_VAR = { todo: "--cat-todo", active: "--cat-active", done: "--cat-done", canceled: "--cat-canceled" };
const PRIORITIES = ["highest", "high", "medium", "low", "lowest"];

// ---------- state ----------
const S = {
  viewer: "", wf: null, issues: [], byId: new Map(), prios: PRIORITIES, etag: null, error: "",
  q: "", type: "", repo: "", who: "", epic: "", pulse: "", closedWin: null, teamCw: "14d",
  selected: null, expanded: new Set(), bodies: new Map(), writing: false, writeId: null, polling: false,
  opener: null, drawerSig: "", bannerSig: "", syncedAt: null, offlineSince: null,
};
function layout() {
  const q = new URLSearchParams(location.search).get("layout");
  if (LAYOUTS[q]) return q;
  try { const saved = localStorage.getItem("orbit.layout"); if (LAYOUTS[saved]) return saved; } catch {}
  return "columns";
}

// ---------- server calls ----------
function api(path, { body, headers } = {}) {
  const init = { cache: "no-store", headers: { "X-Orbit-Token": TOKEN, ...headers } };
  if (body !== undefined) {
    init.method = "POST";
    init.headers["Content-Type"] = "application/json";
    init.body = JSON.stringify(body);
  }
  return fetch(path, init);
}
async function refresh(flashId) {
  try {
    const r = await api("/api/snapshot", { headers: S.etag ? { "If-None-Match": S.etag } : {} });
    if (r.status === 304) { S.syncedAt = new Date(); S.offlineSince = null; setBanner(""); paintSync(); return; }
    if (!r.ok) {
      if (r.status === 401) { forgetToken(); return; }
      S.offlineSince ??= Date.now();
      setBanner(`The Board snapshot failed (HTTP ${r.status}). Retrying.`);
      paintSync();
      return;
    }
    const d = await r.json();
    S.etag = r.headers.get("ETag");
    setData(d);
    S.syncedAt = new Date(); S.offlineSince = null;
    setBanner("");
    renderControls();
    render(flashId);
    paintSync();
  } catch {
    S.offlineSince ??= Date.now();
    setBanner("Lost the connection to orbit board view. Is the command still running?");
    paintSync();
  }
}
const hm = (d) => d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
const hms = (d) => d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
function paintSync() {
  $("src").textContent = S.offlineSince != null
    ? `Offline since ${hm(new Date(S.offlineSince))} — retrying`
    : S.syncedAt ? `Live · synced ${hms(S.syncedAt)}` : "";
}
// One banner slot: a connection error wins, else an active-filter notice.
// Unchanged content keeps the node, so the alert is not re-announced and focus stays on its button.
function setBanner(error) {
  S.error = error;
  const node = error ? h("div", { class: "banner", role: "alert" }, error) : filterBanner();
  const sig = node ? `${error ? "error" : "filter"}|${node.textContent}` : "";
  if (sig === S.bannerSig) return;
  S.bannerSig = sig;
  $("bannerSlot").replaceChildren(...(node ? [node] : []));
}
function activeFilters() {
  return S.q !== "" || S.type !== "" || S.repo !== "" || S.who !== "" || S.epic !== "" || S.pulse !== "";
}
function clearFilters() {
  S.q = ""; S.type = ""; S.repo = ""; S.who = ""; S.epic = ""; S.pulse = "";
  renderControls(); render();
}
function filterBanner() {
  if (S.error || !activeFilters()) return null;
  const n = S.issues.filter((i) => !passFilters(i)).length;
  if (!n) return null;
  return h("div", { class: "banner" },
    visible().length ? `${n} ${n === 1 ? "Issue" : "Issues"} hidden by filters` : "No Issues match these filters.",
    h("button", { class: "btn", onclick: clearFilters }, "Clear filters"));
}
function setData(d) {
  S.teamCw = cwValid(d.closed_window) ? d.closed_window : "14d";
  S.viewer = d.viewer; S.wf = d.workflow; S.issues = d.issues || [];
  for (const i of S.issues) i.key = i.key || i.id; // Boards without a project key
  S.byId = new Map(S.issues.map((i) => [i.id, i]));
  S.prios = [...PRIORITIES];
  for (const i of S.issues) if (!S.prios.includes(i.priority)) S.prios.push(i.priority);
}
// Polling pauses while the viewer is mid-gesture so a re-render never pulls the card away.
const busy = () => S.writing || document.hidden || !!menuEl || $("dlg").open || !!document.querySelector(".card.dragging");
async function poll() {
  if (!TOKEN || S.polling || busy()) return;
  S.polling = true;
  try { await refresh(); } finally { S.polling = false; }
}

// POST a write; follow CHOOSE_TRANSITION and CONFIRM_REQUIRED, then refresh.
async function send(route, i, extra, done) {
  const body = { id: i.id, version: i.updated_at, ...extra };
  S.writing = true; S.writeId = i.id;
  render();
  try {
    for (let step = 0; step < 4; step++) {
      let r, d;
      try {
        r = await api(`/api/${route}`, { body });
        d = await r.json().catch(() => ({}));
      } catch {
        toast("Lost the connection to orbit board view. Nothing was written.", true);
        return;
      }
      if (r.ok) { toast(done); await refresh(i.id); return; }
      if (r.status === 401) { forgetToken(); toast("Nothing was written: the token is no longer valid.", true); return; }
      const e = d.error || {};
      if (e.code === "CHOOSE_TRANSITION" && !body.transition) {
        const t = await chooseDlg(i, e.choices || []);
        if (!t) return;
        body.transition = t;
      } else if (e.code === "CONFIRM_REQUIRED" && !body.confirmed) {
        if (!(await confirmDlg(e.confirm))) return;
        body.confirmed = true;
      } else if (e.code === "VERSION_CONFLICT") {
        await refresh(i.id);
        toast(conflictText(i), true);
        return;
      } else {
        toast(e.detail || e.message || `The write failed (HTTP ${r.status}).`, true);
        await refresh();
        return;
      }
    }
  } finally {
    S.writing = false; S.writeId = null;
    render();
  }
}
// After a conflict the fresh snapshot is already in S; say what moved if it did.
function conflictText(i) {
  const now2 = S.byId.get(i.id), key = i.key || i.id;
  if (!now2) return `${key} changed while you were looking. Nothing was written.`;
  const delta = [];
  if (now2.status !== i.status) delta.push(`status is now ${label(now2.status)}`);
  if ((now2.assignee || "") !== (i.assignee || "")) delta.push(`assignee is now ${now2.assignee || "nobody"}`);
  const what = delta.length ? ` ${delta.join("; ")}.` : "";
  return `${key} changed while you were looking.${what} Nothing was written.`;
}
const moveToColumn = (i, colId) => send("move", i, { column: colId }, `Moved ${i.key} to ${columns().find((c) => c.id === colId)?.name || colId}`);
const moveToStatus = (i, st) => send("move", i, { status: st }, `Moved ${i.key} to ${label(st)}`);
function runStale(i, a) {
  const takeover = a.transition === "claim";
  return send(takeover ? "takeover" : "release", i, {}, takeover ? `You took over ${i.key}` : `Released ${i.key}`);
}
// Buttons that run a write go inert while one is in flight. aria-disabled (not disabled) keeps
// them focusable, so focus survives the re-render; every such onclick checks S.writing.
const writingProps = () => (S.writing ? { "aria-disabled": "true" } : {});

// ---------- workflow + issue helpers ----------
const statusDef = (name) => S.wf.statuses.find((s) => s.name === name);
const catOf = (name) => statusDef(name)?.category;
const label = (name) => (name || "").replace(/_/g, " ").replace(/^./, (c) => c.toUpperCase());
const closedCat = (c) => c === "done" || c === "canceled";
const keyOf = (ref) => S.byId.get(ref)?.key || ref;
const now = () => Date.now();
const claimState = (i) => (i.claim === "live" || i.claim === "stale" ? i.claim : null);
const needsHuman = (i) => i.needs_human || null;
const readyForHuman = (i) => !!i.ready_for_human;
const readyForAgents = (i) => !!i.ready && !i.ready_for_human;
function ago(ms) {
  const m = Math.round(Math.abs(ms) / 60e3);
  if (m < 1) return "<1 min";
  if (m < 60) return `${m} min`;
  const hr = Math.round(m / 60);
  return hr < 48 ? `${hr} h` : `${Math.round(hr / 24)} days`;
}
const clock = (iso) => new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
function timeEl(iso) {
  const d = new Date(iso);
  return h("time", { datetime: iso, title: d.toLocaleString() }, `${ago(now() - d)} ago`);
}
function ancestors(i) {
  const out = []; let p = i.parent;
  while (p && S.byId.has(p) && out.length < 8) { out.push(p); p = S.byId.get(p).parent; }
  return out;
}
// Same order and names as the server's plan::columns.
function columns() {
  const st = S.wf.statuses;
  const ready = st.filter((s) => s.ready).map((s) => s.name);
  const todo = st.filter((s) => s.category === "todo" && !s.ready && s.name !== "blocked").map((s) => s.name);
  const closed = st.filter((s) => closedCat(s.category)).map((s) => s.name);
  return [
    { id: "blocked", name: "Blocked", color: "var(--blocked)", home: null },
    { id: "todo", name: "Todo", color: "var(--cat-todo)", home: todo.includes("todo") ? "todo" : todo[0] },
    { id: "ready", name: label(ready[0] || "ready"), color: "var(--cat-todo)", home: ready[0] },
    ...st.filter((s) => s.category === "active").map((s) => ({ id: s.name, name: label(s.name), color: "var(--cat-active)", home: s.name })),
    { id: "done", name: "Done", color: "var(--cat-done)", home: closed.includes("done") ? "done" : closed[0] },
  ];
}

// ---------- links: blocking edges (open or resolved), related Issues, external references ----------
const targetId = (l) => (typeof l.target === "string" ? l.target : null);
const extRef = (l) => (l.target && typeof l.target === "object" ? `${l.target.system}:${l.target.id}` : null);
const depsOf = (i) => (i.links || []).filter((l) => l.blocking && targetId(l)).map(targetId);
const matches = (ref, x) => ref === x.id || ref === x.key;
const findIssue = (ref) => S.byId.get(ref) || S.issues.find((x) => matches(ref, x));
const dependentsOf = (i) => S.issues.filter((x) => depsOf(x).some((r) => matches(r, i)));
const openDependents = (i) => (closedCat(catOf(i.status)) ? [] : dependentsOf(i).filter((x) => !closedCat(catOf(x.status))));
const relatesOf = (i) => (i.links || []).filter((l) => !l.blocking && targetId(l)).map(targetId);
function relatedOf(i) {
  const out = relatesOf(i).map(findIssue).filter(Boolean);
  for (const x of S.issues) if (x !== i && !out.includes(x) && relatesOf(x).some((r) => matches(r, i))) out.push(x);
  return out;
}
const externalOf = (i) => (i.links || []).map(extRef).filter(Boolean);
const EDGE_TEXT = { up: "blocks it", down: "waits on it", rel: "related", self: "selected Issue" };
function showEdges(i) {
  clearEdges();
  const mark = (x, kind) => document.querySelectorAll(`[data-id="${CSS.escape(x.id)}"]`).forEach((el) => {
    el.dataset.edge = kind;
    (el.matches(".qrow") ? el.querySelector(".qopen") : el)?.setAttribute("aria-description", EDGE_TEXT[kind]);
  });
  const up = depsOf(i).map(findIssue).filter(Boolean), down = dependentsOf(i), rel = relatedOf(i);
  up.forEach((x) => mark(x, "up"));
  down.forEach((x) => mark(x, "down"));
  rel.forEach((x) => mark(x, "rel"));
  if (up.length || down.length || rel.length) mark(i, "self");
}
function clearEdges() {
  document.querySelectorAll("[data-edge]").forEach((el) => { delete el.dataset.edge; });
  document.querySelectorAll("[aria-description]").forEach((el) => el.removeAttribute("aria-description"));
}
const edgeHandlers = (i) => ({ onmouseenter: () => showEdges(i), onmouseleave: clearEdges, onfocus: () => showEdges(i), onblur: clearEdges });
const edgeProps = (i) => ({ "data-id": i.id, ...edgeHandlers(i) });

// ---------- dialogs, menus, toast ----------
// Open the shared dialog; when it closes, focus goes back to whatever opened it.
function showDlg(d) {
  const back = document.activeElement;
  d.addEventListener("close", () => { if (back?.isConnected && back !== document.body) back.focus({ preventScroll: true }); }, { once: true });
  d.showModal();
}
function confirmDlg(c) {
  const d = $("dlg");
  return new Promise((res) => {
    d.replaceChildren(
      h("h3", {}, c.title), h("p", {}, c.body),
      h("div", { class: "acts" },
        h("button", { class: "btn", onclick: () => { res(false); d.close(); } }, "Cancel"),
        h("button", { class: "btn primary", autofocus: true, onclick: () => { res(true); d.close(); } }, c.ok)));
    d.onclose = () => res(false);
    showDlg(d);
  });
}
function chooseDlg(i, choices) {
  const d = $("dlg");
  return new Promise((res) => {
    d.replaceChildren(
      h("h3", {}, `Which transition for ${i.key}?`),
      h("p", {}, "More than one transition reaches that column."),
      h("div", { class: "choices" }, choices.map((c) => h("button", { onclick: () => { res(c.transition); d.close(); } }, c.label, h("small", {}, label(c.transition))))),
      h("div", { class: "acts" }, h("button", { class: "btn", onclick: () => { res(null); d.close(); } }, "Cancel")));
    d.onclose = () => res(null);
    showDlg(d);
  });
}
function shortcutsDlg() {
  const d = $("dlg");
  const row = (k, t) => [h("dt", {}, k), h("dd", {}, t)];
  d.replaceChildren(
    h("h3", {}, "Keyboard shortcuts"),
    h("dl", { class: "keys" },
      row("/", "Focus the search box"),
      row("j / k or ↑ / ↓", "Move between Issues"),
      row("← / →", "Move between columns (Columns layout)"),
      row("Enter", "Open the selected Issue"),
      row("m", "Open the move menu"),
      row("Esc", "Close the menu or detail"),
      row("?", "Show this sheet")),
    h("div", { class: "acts" }, h("button", { class: "btn primary", autofocus: true, onclick: () => d.close() }, "Close")));
  d.onclose = null;
  showDlg(d);
}
const planText = (p) => (p.ok ? (p.actions || []).map((a) => a.label).join(" or ") : p.reason);
let menuEl = null, menuAnchor = null;
// The menu lives on <body>; before a menu item runs a write, park focus on the Issue's card or Queue row
// so the re-render can restore it (saveView tracks .card and .qopen).
function focusMenuIssue() {
  (menuAnchor?.closest(".card") || menuAnchor?.closest(".qrow")?.querySelector(".qopen"))?.focus({ preventScroll: true });
}
function closeMenu(refocus = true) {
  menuEl?.remove(); menuEl = null;
  if (refocus && menuAnchor?.isConnected) menuAnchor.focus();
  menuAnchor = null;
}
function moveMenu(i, anchor) {
  closeMenu(false);
  menuAnchor = anchor;
  const items = [];
  for (const a of i.stale_actions || []) items.push(h("button", { role: "menuitem", "aria-disabled": String(S.writing), onclick: () => { if (!S.writing) { focusMenuIssue(); closeMenu(false); runStale(i, a); } } }, h("span", { class: "dot", style: { "--c": "var(--stale)" } }), a.label));
  for (const s of S.wf.statuses) {
    if (s.name === i.status) continue;
    const p = i.menu?.[s.name] || { ok: false, reason: "" };
    items.push(h("button", { role: "menuitem", "aria-disabled": String(!p.ok || S.writing), onclick: () => { if (p.ok && !S.writing) { focusMenuIssue(); closeMenu(false); moveToStatus(i, s.name); } } },
      h("span", { class: "dot", style: { "--c": `var(${CAT_VAR[s.category]})` } }), `Move to ${label(s.name)}`, h("small", {}, planText(p))));
  }
  menuEl = h("div", { class: "menu", role: "menu", "aria-label": `Move ${i.key}`,
    onkeydown: (e) => {
      const bs = [...menuEl.querySelectorAll("button")];
      const at = bs.indexOf(document.activeElement);
      const step = { ArrowDown: 1, ArrowUp: -1 }[e.key];
      if (step != null) { e.preventDefault(); bs[(at + step + bs.length) % bs.length]?.focus(); }
      else if (e.key === "Home") { e.preventDefault(); bs[0]?.focus(); }
      else if (e.key === "End") { e.preventDefault(); bs[bs.length - 1]?.focus(); }
      else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); closeMenu(); }
    } }, items);
  document.body.append(menuEl);
  const r = anchor.getBoundingClientRect();
  const top = Math.min(r.bottom + 4, innerHeight - menuEl.offsetHeight - 8);
  const left = Math.min(r.left, innerWidth - menuEl.offsetWidth - 8);
  menuEl.style.top = `${Math.max(8, top)}px`;
  menuEl.style.left = `${Math.max(8, left)}px`;
  (menuEl.querySelector('button:not([aria-disabled="true"])') || menuEl.querySelector("button"))?.focus();
}
document.addEventListener("click", (e) => { if (menuEl && !menuEl.contains(e.target) && !e.target.closest("[data-menu]")) closeMenu(false); });
document.addEventListener("keydown", (e) => {
  if ($("dlg").open) return; // the dialog handles its own Esc
  if (e.key === "Escape") {
    if (menuEl) { closeMenu(); return; }
    if ($("drawer").classList.contains("open")) { closeDrawer(); return; }
  }
  if (menuEl) return;
  const t = e.target;
  const typing = t instanceof HTMLElement && (t.tagName === "INPUT" || t.tagName === "SELECT" || t.tagName === "TEXTAREA" || t.isContentEditable);
  if (typing) return;
  if (e.key === "/") { e.preventDefault(); $("filters").querySelector('input[type="search"]')?.focus(); }
  if (e.key === "?") { e.preventDefault(); shortcutsDlg(); }
});
let toastT;
function toast(msg, err) {
  const t = $("toast");
  const back = document.activeElement;
  // Dismissing hides the ✕, so hand focus back to where the viewer was (or the stage).
  const dismiss = () => {
    const hadFocus = t.contains(document.activeElement);
    t.className = "toast";
    if (hadFocus) (back?.isConnected && back !== document.body && !t.contains(back) ? back : $("stage")).focus({ preventScroll: true });
  };
  t.className = `toast show${err ? " err" : ""}`;
  t.setAttribute("role", err ? "alert" : "status");
  t.replaceChildren(h("span", {}, msg), err ? h("button", { "aria-label": "Dismiss", onclick: dismiss }, "✕") : null);
  clearTimeout(toastT);
  if (!err) toastT = setTimeout(() => (t.className = "toast"), 4000);
}

// ---------- shared visual pieces ----------
function ring(i, size = 26) {
  const who = i.assignee, kind = i.assignee_kind;
  const r = size / 2 - 2.5, c = 2 * Math.PI * r, cs = claimState(i);
  let frac = 1, stroke = "var(--line)", dash = null;
  if (cs === "live") { frac = Math.max(.04, Math.min(1, (new Date(i.lease_until) - now()) / LEASE_MS)); stroke = "var(--live)"; }
  if (cs === "stale") { stroke = "var(--stale)"; dash = "3 3"; }
  const initials = who ? who.slice(0, 2).toUpperCase() : "—";
  const tip = !who ? "Unassigned" : cs === "live" ? `${who} (${kind}), lease until ${clock(i.lease_until)}` : cs === "stale" ? `${who} (${kind}), lease expired` : `${who} (${kind || "human"}), no lease`;
  return h("svg:svg", { class: "ring", width: size, height: size, viewBox: `0 0 ${size} ${size}`, role: "img", "aria-label": tip },
    h("svg:title", {}, tip),
    h("svg:circle", { cx: size / 2, cy: size / 2, r, fill: kind === "agent" ? "var(--surface-2)" : "none", stroke: "var(--line)", "stroke-width": 2 }),
    cs ? h("svg:circle", { cx: size / 2, cy: size / 2, r, fill: "none", stroke, "stroke-width": 2.5, "stroke-linecap": "round",
      "stroke-dasharray": dash || `${c * frac} ${c}`, transform: `rotate(-90 ${size / 2} ${size / 2})` }) : null,
    h("svg:text", { x: "50%", y: "50%", "text-anchor": "middle", "dominant-baseline": "central" }, initials));
}
function typeGlyph(t) {
  const col = `var(--type-${{ "sub-task": "sub" }[t] || t}, var(--ink-3))`;
  const sub = t === "sub-task";
  const shape = t === "epic" ? h("svg:path", { d: "M6 1 L11 6 L6 11 L1 6 Z", fill: col })
    : t === "bug" ? h("svg:circle", { cx: 6, cy: 6, r: 4.5, fill: col })
    : t === "story" ? h("svg:path", { d: "M2.5 1.5 H9.5 V10.5 L6 8 L2.5 10.5 Z", fill: col })
    : h("svg:rect", { x: sub ? 2.5 : 1.5, y: sub ? 2.5 : 1.5, width: sub ? 7 : 9, height: sub ? 7 : 9, rx: 2, fill: col });
  return h("svg:svg", { width: 12, height: 12, viewBox: "0 0 12 12", role: "img", "aria-label": t }, h("svg:title", {}, t), shape);
}
function prioGlyph(p) {
  const n = Math.max(1, 4 - Math.max(0, S.prios.indexOf(p)));
  return h("svg:svg", { width: 14, height: 12, viewBox: "0 0 14 12", role: "img", "aria-label": `${p} priority` }, h("svg:title", {}, `${p} priority`),
    [0, 1, 2, 3].map((b) => h("svg:rect", { x: b * 3.5, y: 9 - b * 2.5, width: 2.5, height: 3 + b * 2.5, rx: .6, fill: b < n ? "var(--ink-2)" : "var(--line)" })));
}
function claimBar(i, pfx = "cb") {
  const cs = claimState(i);
  if (cs === "live") {
    const left = new Date(i.lease_until) - now();
    return h("div", { class: "claimbar live", id: `${pfx}-${i.id}` }, `Claimed by ${i.assignee}, ${ago(left)} left`,
      h("span", { class: "leasebar", title: `Lease until ${clock(i.lease_until)}` }, h("i", { style: { width: `${Math.max(0, Math.min(100, left / LEASE_MS * 100))}%` } })));
  }
  if (cs === "stale") return h("div", { class: "claimbar stale", id: `${pfx}-${i.id}` }, `Stale: ${i.assignee}'s lease expired ${ago(now() - new Date(i.lease_until))} ago`);
  return null;
}
// A Stale card's amber claim bar is its band, so it gets no purple one.
const showsHumanBand = (i) => needsHuman(i) && claimState(i) !== "stale";
const humanBar = (i, pfx = "hb") => (showsHumanBand(i) ? h("div", { class: "humanbar", id: `${pfx}-${i.id}` }, needsHuman(i)) : null);
const humanClass = (i) => (showsHumanBand(i) ? "needs-human" : "");
function badges(i, compact, noClaim) {
  const out = [];
  const cs = claimState(i);
  if (!noClaim) {
    if (cs === "stale") out.push(h("span", { class: "chip stale" }, `Stale ${ago(now() - new Date(i.lease_until))}`));
    else if (cs === "live") out.push(h("span", { class: "chip live" }, compact ? "Claimed" : `Claimed until ${clock(i.lease_until)}`));
  }
  const bl = i.blocking || [];
  if (bl.length) out.push(h("span", { class: "chip blocked", title: `Blocked by ${bl.join(", ")}` }, compact ? `Blocked by ${bl.length}` : `Blocked by ${bl.join(", ")}`));
  const waits = openDependents(i);
  if (waits.length) out.push(h("span", { class: "chip deps", title: `${waits.map((x) => x.key).join(", ")} wait on this` }, `Blocks ${waits.length}`));
  const ext = (i.links || []).filter((l) => l.blocking && extRef(l)).map(extRef);
  if (ext.length) out.push(h("span", { class: "chip hold", title: ext.join(", ") }, "Waiting"));
  if (readyForHuman(i)) out.push(h("span", { class: "chip forhuman" }, "For a human"));
  // Stale cards keep their amber bar but also count toward Needs a human, so they carry the violet chip too.
  if (needsHuman(i) && (compact || claimState(i) === "stale")) out.push(h("span", { class: "chip human", title: needsHuman(i) }, "Needs a human"));
  if (i.sub_issue_count) out.push(h("span", { class: "chip", title: "Sub-issues resolved" }, `${i.resolved_sub_issue_count}/${i.sub_issue_count} done`));
  if (i.parent && !compact) out.push(h("span", { class: "chip", title: "Parent" }, `in ${keyOf(i.parent)}`));
  if (i.eligible_for_close) out.push(h("span", { class: "chip live" }, "Ready to close"));
  if (readyForAgents(i) && !compact) out.push(h("span", { class: "chip live" }, "Ready"));
  return out;
}
// ids of the state text a card announces via aria-describedby
function stateIds(i, ...more) {
  const ids = [];
  if (claimState(i)) ids.push(`cb-${i.id}`);
  if (showsHumanBand(i)) ids.push(`hb-${i.id}`);
  ids.push(...more.filter(Boolean));
  return ids.length ? ids.join(" ") : null;
}
function whoEl(i) {
  return h("span", { class: "who" }, ring(i), h("span", {}, i.assignee ? (i.assignee === S.viewer ? `${i.assignee} (you)` : i.assignee) : "Unassigned"));
}
function moreBtn(i) {
  return h("button", { class: "btn ghost more", "data-menu": "1", "aria-label": `Move ${i.key} to another column`, title: "Move to…", ...writingProps(),
    onclick: (e) => { e.stopPropagation(); if (!S.writing) moveMenu(i, e.currentTarget); } }, "⋯");
}
function staleButtons(i) {
  if (claimState(i) !== "stale") return null;
  const acts = i.stale_actions || [];
  return h("div", { class: "staleacts" }, acts.length
    ? acts.map((a) => h("button", { class: a.transition === "claim" ? "btn warn" : "btn", ...writingProps(), onclick: (e) => { e.stopPropagation(); if (!S.writing) runStale(i, a); } }, a.label))
    : h("span", { class: "key" }, "Release it from the CLI."));
}
const dotStyle = (cat) => ({ "--c": `var(${CAT_VAR[cat] || "--cat-todo"})` });

// ---------- detail (drawer in Columns, pane in Queue) ----------
// pfx keeps ids unique: "d" for the drawer, "q" for the Queue pane (both can be in the DOM).
function detailEl(i, extra, pfx = "q") {
  const ck = i.checkpoint;
  const ckEl = ck && h("div", { class: "ckpt" },
    ck.progress && h("div", {}, h("b", {}, "Progress: "), ck.progress),
    ck.next && h("div", {}, h("b", {}, "Next: "), ck.next),
    ck.blocker && h("div", {}, h("b", {}, "Blocker: "), ck.blocker),
    ck.by && h("div", { class: "key" }, ck.at ? `${ck.by}, ${ago(now() - new Date(ck.at))} ago` : ck.by));
  const moves = S.wf.statuses.filter((s) => s.name !== i.status).map((s) => {
    const p = i.menu?.[s.name] || { ok: false, reason: "" };
    return { s, p };
  });
  // When every move is refused for the same reason, say it once above a compact list.
  const sharedReason = moves.length && moves.every((m) => !m.p.ok && m.p.reason && m.p.reason === moves[0].p.reason) ? moves[0].p.reason : null;
  const moveBtns = moves.map(({ s, p }) =>
    h("button", { disabled: !p.ok, ...writingProps(), "data-act": `move-${s.name}`, onclick: () => { if (!S.writing) moveToStatus(i, s.name); } }, h("span", { class: "dot", style: dotStyle(s.category) }),
      `Move to ${label(s.name)}`, sharedReason ? null : h("small", {}, planText(p))));
  return h("div", { class: "detail" },
    h("div", { class: "row" }, typeGlyph(i.type), h("span", { class: "key" }, i.key), extra),
    h("h3", { id: `${pfx}title`, tabindex: -1 }, i.title),
    claimBar(i, `${pfx}-cb`), humanBar(i, `${pfx}-hb`),
    h("div", { class: "row" }, badges(i)),
    staleButtons(i),
    h("dl", {},
      h("dt", {}, "Status"), h("dd", {}, label(i.status)),
      h("dt", {}, "Assignee"), h("dd", {}, whoEl(i)),
      h("dt", {}, "Priority"), h("dd", { class: "row" }, prioGlyph(i.priority), i.priority),
      h("dt", {}, "Type"), h("dd", {}, i.type),
      h("dt", {}, "Repository"), h("dd", {}, i.repository || "Board-wide"),
      i.parent && [h("dt", {}, "Parent"), h("dd", {}, `${keyOf(i.parent)} ${S.byId.get(i.parent)?.title || ""}`)],
      (i.labels || []).length ? [h("dt", {}, "Labels"), h("dd", {}, i.labels.join(", "))] : null,
      h("dt", {}, "Updated"), h("dd", {}, timeEl(i.updated_at))),
    h("section", { class: "bodyslot", "data-body": i.id }, bodyEls(i)),
    linksEl(i),
    ckEl && h("section", {}, h("h4", {}, "Checkpoint"), ckEl),
    subsOf(i).length ? h("section", {}, h("h4", {}, `Sub-issues (${subsOf(i).length})`), h("div", { class: "moves" }, subsOf(i).map((s) =>
      h("button", { "data-act": `sub-${s.id}`, onclick: () => select(s.id) }, h("span", { class: "dot", style: dotStyle(catOf(s.status)) }), `${s.key} ${s.title}`, h("small", {}, label(s.status)))))) : null,
    h("section", {}, h("h4", {}, "Move"),
      sharedReason ? h("p", { class: "note" }, sharedReason) : null,
      h("div", { class: "moves" }, moveBtns)));
}
const subsOf = (i) => S.issues.filter((x) => x.parent === i.id);
function bodyEls(i) {
  const b = S.bodies.get(i.id);
  if (!b || b.version !== i.updated_at) return h("h4", {}, "Loading description…");
  const comments = b.comments || [];
  return [h("h4", {}, "Description"), h("pre", {}, b.body || "No description."),
    comments.length ? [h("h4", { class: "sub" }, `Comments (${comments.length})`), ...comments.slice(-3).map((c) => h("pre", {}, `${c.author}: ${c.body}`))] : null];
}
function linksEl(i) {
  const row = (x) => h("button", { "data-act": `link-${x.id}`, onclick: () => select(x.id), onmouseenter: () => showEdges(x), onmouseleave: clearEdges },
    h("span", { class: "dot", style: dotStyle(catOf(x.status)) }), h("span", { class: "key" }, x.key), h("span", { class: "title" }, x.title), h("small", { class: "key" }, label(x.status)));
  const up = depsOf(i).map(findIssue).filter(Boolean), down = dependentsOf(i), rel = relatedOf(i), ext = externalOf(i);
  if (!up.length && !down.length && !rel.length && !ext.length) return null;
  const grp = (title, xs) => (xs.length ? [h("div", { class: "grp" }, title), xs] : null);
  return h("section", {}, h("h4", {}, "Links"), h("div", { class: "links" },
    grp(`Blocked by (${up.filter((x) => !closedCat(catOf(x.status))).length} open)`, up.map(row)),
    grp("Blocks", down.map(row)),
    grp("Relates to", rel.map(row)),
    grp("External", ext.map((e) => h("div", { class: "ext" }, e)))));
}
async function loadBody(i) {
  const cached = S.bodies.get(i.id);
  if (cached && cached.version === i.updated_at) return;
  try {
    const r = await api(`/api/show/${encodeURIComponent(i.id)}`);
    if (!r.ok) return;
    const d = await r.json();
    S.bodies.set(i.id, { version: i.updated_at, body: d.body || "", comments: d.comments || [] });
    // Narrow Queue shows the drawer and keeps a hidden pane, so patch every slot for this Issue.
    for (const slot of document.querySelectorAll(`.bodyslot[data-body="${CSS.escape(i.id)}"]`)) slot.replaceChildren(...[bodyEls(i)].flat(Infinity).filter(Boolean));
  } catch {}
}
const narrow = () => matchMedia("(max-width: 760px)").matches;
const closeBtn = () => h("button", { class: "btn ghost push", onclick: closeDrawer, "data-act": "close", "aria-label": "Close" }, "✕");
// What about the open drawer Issue would force a rebuild: version, claim text, write state.
function drawerSig(i) {
  const cs = claimState(i);
  const lease = cs === "live" ? ago(new Date(i.lease_until) - now()) : cs === "stale" ? ago(now() - new Date(i.lease_until)) : "";
  return [i.id, i.updated_at, i.status, i.assignee, cs, lease, i.needs_human || "", i.title, S.writing].join("|");
}
function select(id) {
  S.selected = id;
  const i = S.byId.get(id);
  if (!i) return;
  if (layout() === "queue" && !narrow()) { render(); return; }
  const d = $("drawer");
  d.replaceChildren(detailEl(i, closeBtn(), "d"));
  S.drawerSig = drawerSig(i);
  d.classList.add("open");
  loadBody(i);
  d.querySelector("h3")?.focus();
}
function closeDrawer() {
  const d = $("drawer");
  const was = d.classList.contains("open");
  d.classList.remove("open");
  S.drawerSig = "";
  if (layout() !== "queue") S.selected = null;
  if (was && S.opener) {
    const back = document.querySelector(`#stage .card[data-id="${CSS.escape(S.opener)}"], #stage .qopen[data-open="${CSS.escape(S.opener)}"]`);
    if (back) back.focus({ preventScroll: true });
    S.opener = null;
  }
}

// ---------- filtering ----------
function passFilters(i) {
  const q = S.q.toLowerCase();
  if (q && !`${i.key} ${i.title} ${(i.labels || []).join(" ")} ${i.assignee || ""}`.toLowerCase().includes(q)) return false;
  if (S.type && i.type !== S.type) return false;
  if (S.repo && (i.repository || "(board-wide)") !== S.repo) return false;
  if (S.who === "~none" && i.assignee) return false;
  if (S.who && S.who !== "~none" && (i.assignee || "") !== (S.who === "~me" ? S.viewer : S.who)) return false;
  if (S.epic && i.id !== S.epic && !ancestors(i).includes(S.epic)) return false;
  if (S.pulse && !PULSES[S.pulse].test(i)) return false;
  return true;
}
// ---------- Closed window (ADR 0023 amendment) ----------
// closedWin is the session override: a spec string ("all" from the older footer) or a custom {from, to}; the saved choice and team default sit behind it.
const cwValid = (v) => typeof v === "string" && (v === "all" || /^[1-9]\d*[hdw]$/.test(v));
function cwSaved() { try { const v = localStorage.getItem(CW_KEY); return cwValid(v) ? v : null; } catch { return null; } }
const cwSpec = () => (typeof S.closedWin === "string" ? S.closedWin : cwSaved() ?? S.teamCw);
function cwLabel(v) {
  const p = CW_PRESETS.find(([k]) => k === v);
  if (p) return p[1];
  const n = parseInt(v, 10);
  const unit = { h: "hour", d: "day", w: "week" }[v.slice(-1)];
  return `${n} ${unit}${n === 1 ? "" : "s"}`;
}
// A closed Issue's moment: resolved_at, else updated_at for Issues closed before it was recorded (ADR 0024).
const closedAt = (i) => new Date(i.resolved_at || i.updated_at);
function inWindow(i) {
  const w = S.closedWin;
  if (w && typeof w === "object") {
    const t = closedAt(i);
    return t >= new Date(`${w.from}T00:00:00`) && t < new Date(`${w.to}T00:00:00`).getTime() + 864e5;
  }
  const spec = cwSpec();
  return spec === "all" || now() - closedAt(i) <= parseInt(spec, 10) * CW_UNIT_MS[spec.slice(-1)];
}
// A text search that matches shows the Issue whatever the window says.
const hiddenOlder = (i) => closedCat(catOf(i.status)) && !S.q && !inWindow(i);
function visible() {
  return S.issues.filter((i) => passFilters(i) && !hiddenOlder(i));
}
const cwText = () => (S.closedWin && typeof S.closedWin === "object" ? `${S.closedWin.from} to ${S.closedWin.to}` : cwLabel(cwSpec()));
// Reveals what the window hides, for this session only.
const olderFooter = () => { const n = olderCount(); return n ? h("button", { class: "btn ghost older", onclick: () => { S.closedWin = "all"; renderClosedWin(); render(); } }, `+${n} older · show`) : null; };
const olderCount = () => S.issues.filter((i) => passFilters(i) && hiddenOlder(i)).length;
function setClosedWin(v) {
  if (v === "custom") {
    const iso = (d) => { const z = new Date(d - d.getTimezoneOffset() * 6e4); return z.toISOString().slice(0, 10); };
    S.closedWin = { from: iso(new Date(now() - 7 * 864e5)), to: iso(new Date(now())) };
  } else {
    S.closedWin = null;
    try { localStorage.setItem(CW_KEY, v); } catch {}
  }
  renderClosedWin(); render();
}
function resetClosedWin() {
  S.closedWin = null;
  try { localStorage.removeItem(CW_KEY); } catch {}
  renderClosedWin(); render();
}
function renderClosedWin() {
  const custom = S.closedWin && typeof S.closedWin === "object";
  const saved = cwSaved();
  const spec = cwSpec();
  const known = new Set(CW_PRESETS.map(([k]) => k));
  const extras = [...new Set([S.teamCw, saved, spec])].filter((v) => v && !known.has(v));
  const opt = (v, t) => h("option", { value: v, selected: !custom && v === spec }, t);
  const range = (k) => h("input", { type: "date", "aria-label": k === "from" ? "Closed from" : "Closed to", value: S.closedWin[k],
    onchange: (e) => { if (e.target.value) { S.closedWin[k] = e.target.value; render(); } } });
  $("closedwin").replaceChildren(...[
    h("label", { for: "cwsel" }, "Closed:"),
    h("select", { id: "cwsel", onchange: (e) => setClosedWin(e.target.value) },
      CW_PRESETS.map(([k, t]) => opt(k, t)),
      extras.map((v) => opt(v, `${cwLabel(v)}${v === S.teamCw ? " (team default)" : ""}`)),
      h("option", { value: "custom", selected: !!custom }, "Custom…")),
    custom ? [range("from"), "–", range("to")] : null,
    custom || saved || S.closedWin ? h("button", { class: "btn ghost", onclick: resetClosedWin, title: `Use the team default (${cwLabel(S.teamCw)})` }, "Reset") : null,
  ].flat().filter(Boolean));
}
// The header pills; one at a time filters the board.
const PULSES = {
  agents: { text: "ready for agents", test: readyForAgents },
  forhuman: { text: "ready for a human", test: readyForHuman },
  human: { test: (i) => !!needsHuman(i), tone: "human" },
  active: { text: "active", test: (i) => catOf(i.status) === "active" },
  claimed: { text: "claimed", test: (i) => claimState(i) === "live" },
  stale: { text: "stale", test: (i) => claimState(i) === "stale", tone: "warn" },
  close: { text: "ready to close", test: (i) => !!i.eligible_for_close },
};
const byPrio = (a, b) => S.prios.indexOf(a.priority) - S.prios.indexOf(b.priority) || (a.key < b.key ? -1 : 1);

// ---------- drag and drop (targets and reasons come from each Issue's `moves`) ----------
let dragId = null;
function dragStart(e, i) {
  dragId = i.id; e.dataTransfer.effectAllowed = "move"; e.dataTransfer.setData("text/plain", i.key);
  e.currentTarget.classList.add("dragging");
  closeMenu(false);
  document.querySelectorAll("[data-drop]").forEach((z) => {
    const col = z.dataset.drop, same = col === i.column;
    const p = i.moves?.[col] || { ok: false, reason: "" };
    z.classList.toggle("target", !!p.ok);
    z.classList.toggle("nope", !p.ok && !same);
    const why = z.querySelector(".why"); if (why) why.textContent = p.ok || same ? "" : p.reason;
  });
}
function dragEnd(e) {
  dragId = null; e.currentTarget.classList.remove("dragging");
  document.querySelectorAll("[data-drop]").forEach((z) => { z.classList.remove("target", "nope", "over"); const why = z.querySelector(".why"); if (why) why.textContent = z.dataset.idle || ""; });
}
function dropZone(el, colId) {
  el.dataset.drop = colId;
  el.addEventListener("dragover", (e) => { if (el.classList.contains("target")) { e.preventDefault(); el.classList.add("over"); } });
  el.addEventListener("dragleave", () => el.classList.remove("over"));
  el.addEventListener("drop", (e) => { e.preventDefault(); const i = S.byId.get(dragId); if (i && el.classList.contains("target")) moveToColumn(i, colId); });
  return el;
}
function draggableProps(i) {
  const movable = !claimState(i) && i.column !== "blocked" && !S.writing;
  return { draggable: movable ? "true" : "false", ondragstart: movable ? (e) => dragStart(e, i) : null, ondragend: movable ? dragEnd : null };
}
const stateClass = (i) => ({ live: "is-live", stale: "is-stale" })[claimState(i)] || "";

// ---------- Columns layout ----------
function cardEl(i, col) {
  const cancelled = catOf(i.status) === "canceled";
  // In Blocked, "Status: ready" so the chip is not read as the Ready condition.
  const chip = cancelled ? "Cancelled" : col.id === "blocked" ? statusText(i) : i.status !== col.home ? label(i.status) : null;
  const foot = badges(i, false, true);
  const badgeId = foot.length ? `bd-${i.id}` : null;
  const why = col.id === "ready" ? notReadyReason(i) : null;
  const whyId = why ? `nr-${i.id}` : null;
  const open = () => { S.opener = i.id; select(i.id); };
  return h("article", { class: `card ${stateClass(i)} ${humanClass(i)} ${cancelled ? "cancelled" : ""} ${S.writeId === i.id ? "busy" : ""}`, tabindex: 0,
    "aria-label": `${i.key}: ${i.title}`, "aria-describedby": stateIds(i, whyId, badgeId),
    ...edgeProps(i), ...draggableProps(i),
    onclick: open, onkeydown: (e) => { if (e.target !== e.currentTarget) return; if (e.key === "Enter") open(); if (e.key === "m") moveMenu(i, e.currentTarget); } },
    claimBar(i), humanBar(i),
    h("div", { class: "head" }, typeGlyph(i.type), h("span", { class: "key" }, i.key), prioGlyph(i.priority), chip ? h("span", { class: "chip status" }, chip) : null, moreBtn(i)),
    h("div", { class: "title" }, i.title),
    why ? h("div", { class: "notready", id: whyId }, `Not ready: ${why}`) : null,
    h("div", { class: "foot" }, whoEl(i)),
    foot.length ? h("div", { class: "foot", id: badgeId }, foot) : null,
    staleButtons(i));
}
const statusText = (i) => `Status: ${label(i.status).toLowerCase()}`;
// Why a card in the Ready column is not in `orbit ready`; null when it is, or when its For a human tag says so.
function notReadyReason(i) {
  if (i.ready) return null;
  const has = (l) => (i.labels || []).includes(l);
  if (has("needs-triage")) return "needs triage";
  if (has("needs-info")) return "needs info";
  if (has("ready-for-human")) return readyForHuman(i) ? null : "for a human";
  const subs = i.open_sub_issue_count || 0;
  if (subs) return `${subs} open sub-${subs === 1 ? "issue" : "issues"}`;
  if (i.assignee) return `assigned to ${i.assignee}`;
  return "not in the agents' queue";
}
// Arrow keys (or j/k) walk the grid of cards; Enter opens, m opens the move menu.
function colsKeys(e) {
  const card = e.target.closest?.(".card");
  if (!card || e.target !== card) return;
  const list = card.parentElement;
  const cards = [...list.querySelectorAll(".card")];
  const at = cards.indexOf(card);
  const step = { ArrowDown: 1, ArrowUp: -1, j: 1, k: -1 }[e.key];
  if (step != null) { e.preventDefault(); (cards[at + step] || card).focus(); return; }
  const side = { ArrowRight: 1, ArrowLeft: -1 }[e.key];
  if (side != null) {
    e.preventDefault();
    const cols = [...document.querySelectorAll(".col")];
    const ci = cols.indexOf(card.closest(".col"));
    for (let n = ci + side; n >= 0 && n < cols.length; n += side) {
      const next = cols[n].querySelectorAll(".card");
      if (next.length) { next[Math.min(at, next.length - 1)].focus(); return; }
    }
  }
}
function renderColumns(stage, list) {
  const cols = columns().map((col) => {
    const items = list.filter((i) => i.column === col.id).sort(byPrio);
    const style = { "--c": col.color };
    if (col.id === "done" && !S.expanded.has("done")) {
      const show = () => { S.expanded.add("done"); render(); };
      return dropZone(h("section", { class: "col collapsed", style, "aria-label": `Done, ${items.length} Issues` },
        h("button", { class: "colbtn", onclick: show, "aria-label": `Done, ${items.length} Issues, show` },
          h("div", { class: "vlabel" }, `Done ${items.length}`))), col.id);
    }
    const idle = col.id === "blocked" && !items.length ? "Nothing is blocked." : "";
    const zone = dropZone(h("section", { class: "col", style, "aria-label": col.name },
      h("header", {}, h("h2", {}, col.name),
        h("span", { class: "n" }, col.id === "ready" ? `${items.length} · ${items.filter(readyForAgents).length} for agents` : items.length),
        col.id === "done" ? h("button", { class: "btn ghost push", onclick: () => { S.expanded.delete("done"); render(); } }, "Hide") : null),
      h("p", { class: "why" }, idle),
      h("div", { class: "list" }, items.map((i) => cardEl(i, col)), col.id === "done" ? olderFooter() : null)), col.id);
    zone.dataset.idle = idle;
    return zone;
  });
  stage.replaceChildren(h("div", { class: "cols", onkeydown: colsKeys }, cols));
}

// ---------- Queue layout ----------
function renderQueue(stage, list) {
  const human = list.filter((i) => needsHuman(i));
  const rest = list.filter((i) => !needsHuman(i));
  const inCol = (id) => rest.filter((i) => i.column === id);
  const sections = [
    ["Needs a human", "Nothing moves until a person takes the step named on each Issue.", human.sort(byPrio)],
    ["In progress", "Being worked by agents or people. Stale or human-blocked active Issues are listed under Needs a human.", rest.filter((i) => catOf(i.status) === "active").sort(byPrio)],
    ["Up next", "Agents take these in order; Issues marked For a human wait for a person.", inCol("ready").sort((a, b) => readyForHuman(a) - readyForHuman(b) || byPrio(a, b))],
    ["Blocked", "They move to Up next when their blockers close.", inCol("blocked").sort(byPrio)],
    ["Todo", "Not ready to start yet.", inCol("todo").sort(byPrio)],
    ["Closed recently", `Closed in the window: ${cwText()}.`, inCol("done").sort((a, b) => b.updated_at.localeCompare(a.updated_at))],
  ];
  const flat = sections.flatMap((s) => s[2]);
  if (!S.selected || !flat.some((i) => i.id === S.selected)) S.selected = flat[0]?.id || null;
  const older = olderCount();
  const rows = sections.filter((s) => s[2].length || (s[0] === "Closed recently" && older)).map(([title, hint, xs]) => h("section", { class: "qsec" },
    h("h2", {}, title, h("span", { class: "n" }, xs.length)), h("p", { class: "hint" }, hint),
    title === "Closed recently" ? olderFooter() : null,
    xs.map((i) => {
      const sel = S.selected === i.id;
      // The badges sit inside the button (its name); describe it only with what lives outside it.
      const humanId = needsHuman(i) ? `qh-${i.id}` : null;
      return h("div", { class: `qrow ${stateClass(i)} ${humanClass(i)}`, ...edgeProps(i) },
        h("button", { class: "qopen", "data-open": i.id, tabindex: sel ? 0 : -1, "aria-current": sel ? "true" : null, ...edgeHandlers(i),
          "aria-describedby": humanId,
          onclick: () => { S.opener = i.id; select(i.id); } },
          h("span", { class: "keycell", title: i.key }, prioGlyph(i.priority), h("span", { class: "key" }, i.key)),
          h("span", { class: "title", title: i.title }, typeGlyph(i.type), " ", i.title),
          h("span", { class: "badges" }, badges(i, true)),
          whoEl(i)),
        humanId ? h("span", { class: "sr-only", id: humanId }, needsHuman(i)) : null,
        h("button", { class: "statusbtn", "data-menu": "1", style: dotStyle(catOf(i.status)), "aria-label": `${i.key}: ${label(i.status)}, move`, ...writingProps(),
          onclick: (e) => { e.stopPropagation(); if (!S.writing) moveMenu(i, e.currentTarget); } }, h("span", { class: "dot" }), i.column === "blocked" ? statusText(i) : label(i.status)));
    })));
  const selIssue = S.byId.get(S.selected);
  const empty = h("p", { class: "qempty" }, "No Issues match these filters.",
    activeFilters() ? h("div", {}, h("button", { class: "btn", onclick: clearFilters }, "Clear filters")) : null);
  stage.replaceChildren(h("div", { class: "queue" },
    h("div", { class: "qlist", "aria-label": "Issues", onkeydown: qKeys }, rows.length ? rows : empty),
    h("div", { class: "qdetail" }, selIssue ? detailEl(selIssue) : h("p", { class: "qempty" }, "Select an Issue."))));
  if (selIssue) loadBody(selIssue);
}
function qKeys(e) {
  const inRow = e.target.closest?.(".qrow");
  if (!inRow) return;
  const btns = [...document.querySelectorAll(".qopen")];
  const ids = btns.map((b) => b.dataset.open);
  const at = ids.indexOf(S.selected);
  const go = (n) => {
    e.preventDefault();
    S.selected = ids[Math.max(0, Math.min(ids.length - 1, at + n))];
    render();
    document.querySelector('.qopen[aria-current="true"]')?.focus();
  };
  if (e.key === "j" || e.key === "ArrowDown") go(1);
  if (e.key === "k" || e.key === "ArrowUp") go(-1);
  // Enter is the button's native click; ←/→ have no meaning in a single list.
  if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && e.target.classList.contains("qopen")) e.preventDefault();
  if (e.key === "m") {
    const row = document.querySelector('.qopen[aria-current="true"]')?.closest(".qrow");
    if (row) moveMenu(S.byId.get(S.selected), row.querySelector(".statusbtn") || row.querySelector(".qopen"));
  }
}

// ---------- chrome: pulse counts and filters ----------
function renderControls() {
  const all = S.issues;
  $("pulse").replaceChildren(...Object.entries(PULSES).map(([k, p]) => {
    const n = all.filter(p.test).length;
    const text = k === "human" ? `${n === 1 ? "needs" : "need"} a human` : p.text;
    return h("button", { class: n && p.tone ? p.tone : "", "aria-pressed": String(S.pulse === k), onclick: () => { S.pulse = S.pulse === k ? "" : k; renderControls(); render(); } }, h("b", {}, n), text);
  }));
  $("me").replaceChildren(ring({ assignee: S.viewer, assignee_kind: "human" }), `Acting as ${S.viewer}`);
  // Rebuilding the filters while the viewer types would drop focus and the caret.
  renderClosedWin();
  if ($("filters").contains(document.activeElement)) return;
  const opt = (v, t, cur) => h("option", { value: v, selected: v === cur }, t);
  const types = [...new Set(all.map((i) => i.type))].sort();
  const repos = [...new Set(all.map((i) => i.repository || "(board-wide)"))].sort();
  const epics = all.filter((i) => i.type === "epic").sort(byPrio);
  $("filters").replaceChildren(
    h("input", { type: "search", placeholder: "Filter by key, title, label", value: S.q, "aria-label": "Filter", oninput: (e) => { S.q = e.target.value; render(); } }),
    h("select", { "aria-label": "Type", onchange: (e) => { S.type = e.target.value; render(); } }, opt("", "All types", S.type), types.map((t) => opt(t, t, S.type))),
    h("select", { "aria-label": "Assignee", onchange: (e) => { S.who = e.target.value; render(); } },
      opt("", "All assignees", S.who), opt("~me", `Me (${S.viewer})`, S.who), opt("~none", "Unassigned", S.who),
      ["agent", "human"].map((kind) => {
        const names = [...new Set(all.filter((i) => i.assignee && i.assignee !== S.viewer && (i.assignee_kind || "human") === kind).map((i) => i.assignee))].sort();
        return names.length ? h("optgroup", { label: kind === "agent" ? "Agents" : "People" }, names.map((n) => opt(n, n, S.who))) : null;
      })),
    h("select", { "aria-label": "Repository", onchange: (e) => { S.repo = e.target.value; render(); } }, opt("", "All repositories", S.repo), repos.map((r) => opt(r, r, S.repo))),
    h("select", { "aria-label": "Epic", onchange: (e) => { S.epic = e.target.value; render(); } }, opt("", "All epics", S.epic),
      epics.map((x) => opt(x.id, `${x.key} ${x.title.length > 40 ? `${x.title.slice(0, 40)}…` : x.title}`, S.epic))));
}

// ---------- focus / scroll preservation ----------
// A re-render replaces the stage; keep the viewer's scroll positions and focused card or row.
const scrollKey = (el) => `${el.className}|${el.closest("[data-drop]")?.dataset.drop || el.querySelector("[data-body]")?.dataset.body || ""}`;
// A stable handle for a focusable element inside a rebuilt container.
const focusKey = (a) => [a.tagName, a.dataset.act || "", a.getAttribute("aria-label") || "", a.className, a.textContent.trim().slice(0, 40)].join("|");
function saveView() {
  const a = document.activeElement;
  const focus = a && $("stage").contains(a) && a.matches(".card, .qopen") ? a.dataset.id || a.dataset.open : null;
  const scroll = new Map([...document.querySelectorAll("#stage .cols, #stage .list, #stage .qlist, #stage .qdetail")].map((el) => [scrollKey(el), [el.scrollLeft, el.scrollTop]]));
  return { focus, scroll };
}
function restoreView({ focus, scroll }) {
  for (const el of document.querySelectorAll("#stage .cols, #stage .list, #stage .qlist, #stage .qdetail")) {
    const at = scroll.get(scrollKey(el));
    if (at) [el.scrollLeft, el.scrollTop] = at;
  }
  if (focus) $("stage").querySelector(`.card[data-id="${CSS.escape(focus)}"], .qopen[data-open="${CSS.escape(focus)}"]`)?.focus({ preventScroll: true });
}
// Focused control inside a rebuilt pane (drawer or Queue detail): match it again and keep scrollTop.
function saveInner(root) {
  const a = document.activeElement;
  const inRoot = root.contains(a) && a !== root && a !== document.body;
  return { key: inRoot ? focusKey(a) : null, top: root.scrollTop };
}
function restoreInner(root, saved) {
  root.scrollTop = saved.top;
  if (!saved.key) return;
  for (const el of root.querySelectorAll("button, a, input, [tabindex]")) {
    if (focusKey(el) === saved.key) { el.focus({ preventScroll: true }); return; }
  }
  // The control is gone (say the move it ran succeeded): land on the title, not <body>.
  root.querySelector("h3[tabindex]")?.focus({ preventScroll: true });
}

function render(flashId) {
  if (!S.wf) return;
  const v = layout();
  $("layout").replaceChildren(...Object.entries(LAYOUTS).map(([k, name]) => h("button", { role: "radio", "aria-checked": String(k === v), onclick: () => setLayout(k) }, name)));
  const list = visible();
  const view = saveView();
  const d = $("drawer");
  const dOpen = S.selected && d.classList.contains("open");
  // The Queue detail pane lives inside the stage; save its inner focus too.
  const qdetail = document.querySelector("#stage .qdetail");
  const qFocus = qdetail ? saveInner(qdetail) : null;
  const dFocus = dOpen ? saveInner(d) : null;
  if (v === "queue") renderQueue($("stage"), list); else renderColumns($("stage"), list);
  if (qFocus && qFocus.key) {
    const pane = document.querySelector("#stage .qdetail");
    if (pane) restoreInner(pane, qFocus);
  }
  if (dOpen) {
    const i = S.byId.get(S.selected);
    if (i) {
      const sig = drawerSig(i);
      if (sig !== S.drawerSig) {
        d.replaceChildren(detailEl(i, closeBtn(), "d"));
        S.drawerSig = sig;
        restoreInner(d, dFocus);
      }
      loadBody(i);
    } else closeDrawer();
  }
  restoreView(view);
  if (flashId) document.querySelector(`[data-id="${CSS.escape(flashId)}"]`)?.classList.add("flash");
  setBanner(S.error);
}

// ---------- layout switch ----------
function setLayout(name) {
  try { localStorage.setItem("orbit.layout", name); } catch {}
  const u = new URL(location.href); u.searchParams.set("layout", name); history.replaceState(null, "", u);
  closeDrawer(); closeMenu(false); render();
}

// ---------- start ----------
$("shortcuts").addEventListener("click", shortcutsDlg);
if (!TOKEN) setBanner(NO_TOKEN);
else refresh();
setInterval(poll, POLL_MS);
setInterval(() => { if (!busy()) render(); }, 30e3); // lease rings and "min left" tick between snapshot changes
