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

// ---------- token: read once from the fragment, keep in memory, strip from the address bar ----------
const TOKEN = (/(?:^#|&)token=([0-9a-f]{64})(?:&|$)/.exec(location.hash) || [])[1] || "";
if (location.hash) history.replaceState(null, "", location.pathname + location.search);

// ---------- constants ----------
const LAYOUTS = { columns: "Columns", queue: "Queue" };
const POLL_MS = 3000;
const LEASE_MS = 30 * 60e3;
const CLOSED_WINDOW_MS = 14 * 864e5;
const CAT_VAR = { todo: "--cat-todo", active: "--cat-active", done: "--cat-done", canceled: "--cat-canceled" };
const PRIORITIES = ["highest", "high", "medium", "low", "lowest"];

// ---------- state ----------
const S = {
  viewer: "", wf: null, issues: [], byId: new Map(), prios: PRIORITIES, etag: null, error: "",
  q: "", type: "", repo: "", who: "", epic: "", pulse: "", showAllClosed: false,
  selected: null, expanded: new Set(), bodies: new Map(), writing: false, polling: false,
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
    if (r.status === 304) return;
    if (!r.ok) {
      setError(r.status === 401 || r.status === 403
        ? "This page has no valid token. Open the link that orbit board view printed (a reload drops the token)."
        : `The Board snapshot failed (HTTP ${r.status}). Retrying.`);
      return;
    }
    const d = await r.json();
    S.etag = r.headers.get("ETag");
    setData(d);
    setError("");
    renderControls();
    render(flashId);
  } catch {
    setError("Lost the connection to orbit board view. Is the command still running?");
  }
}
function setError(msg) {
  if (S.error === msg) return;
  S.error = msg;
  $("bannerSlot").replaceChildren(msg ? h("div", { class: "banner", role: "alert" }, msg) : "");
}
function setData(d) {
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
  S.writing = true;
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
      const e = d.error || {};
      if (e.code === "CHOOSE_TRANSITION" && !body.transition) {
        const t = await chooseDlg(i, e.choices || []);
        if (!t) return;
        body.transition = t;
      } else if (e.code === "CONFIRM_REQUIRED" && !body.confirmed) {
        if (!(await confirmDlg(e.confirm))) return;
        body.confirmed = true;
      } else if (e.code === "VERSION_CONFLICT") {
        toast(`${i.key || i.id} changed while you were looking. Nothing was written.`, true);
        await refresh();
        return;
      } else {
        toast(e.detail || e.message || `The write failed (HTTP ${r.status}).`, true);
        await refresh();
        return;
      }
    }
  } finally {
    S.writing = false;
  }
}
const moveToColumn = (i, colId) => send("move", i, { column: colId }, `Moved ${i.key} to ${columns().find((c) => c.id === colId)?.name || colId}`);
const moveToStatus = (i, st) => send("move", i, { status: st }, `Moved ${i.key} to ${label(st)}`);
function runStale(i, a) {
  const takeover = a.label === "Take over";
  return send(takeover ? "takeover" : "release", i, {}, takeover ? `You took over ${i.key}` : `Released ${i.key}`);
}

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
  if (m < 60) return `${m} min`;
  const hr = Math.round(m / 60);
  return hr < 48 ? `${hr} h` : `${Math.round(hr / 24)} days`;
}
const clock = (iso) => new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
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
    { id: "blocked", name: "Blocked", color: "var(--blocked)" },
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
function showEdges(i) {
  clearEdges();
  const mark = (x, kind) => document.querySelectorAll(`[data-id="${CSS.escape(x.id)}"]`).forEach((el) => (el.dataset.edge = kind));
  const up = depsOf(i).map(findIssue).filter(Boolean), down = dependentsOf(i), rel = relatedOf(i);
  up.forEach((x) => mark(x, "up"));
  down.forEach((x) => mark(x, "down"));
  rel.forEach((x) => mark(x, "rel"));
  if (up.length || down.length || rel.length) mark(i, "self");
}
function clearEdges() { document.querySelectorAll("[data-edge]").forEach((el) => delete el.dataset.edge); }
const edgeProps = (i) => ({ "data-id": i.id, onmouseenter: () => showEdges(i), onmouseleave: clearEdges, onfocus: () => showEdges(i), onblur: clearEdges });

// ---------- dialogs, menus, toast ----------
function confirmDlg(c) {
  const d = $("dlg");
  return new Promise((res) => {
    d.replaceChildren(
      h("h3", {}, c.title), h("p", {}, c.body),
      h("div", { class: "acts" },
        h("button", { class: "btn", onclick: () => { res(false); d.close(); } }, "Cancel"),
        h("button", { class: "btn primary", autofocus: true, onclick: () => { res(true); d.close(); } }, c.ok)));
    d.onclose = () => res(false);
    d.showModal();
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
    d.showModal();
  });
}
const planText = (p) => (p.ok ? (p.actions || []).map((a) => a.label).join(" or ") : p.reason);
let menuEl = null;
function closeMenu() { menuEl?.remove(); menuEl = null; }
function moveMenu(i, anchor) {
  closeMenu();
  const items = [];
  for (const a of i.stale_actions || []) items.push(h("button", { role: "menuitem", onclick: () => { closeMenu(); runStale(i, a); } }, h("span", { class: "dot", style: { "--c": "var(--stale)" } }), a.label));
  for (const s of S.wf.statuses) {
    if (s.name === i.status) continue;
    const p = i.menu?.[s.name] || { ok: false, reason: "" };
    items.push(h("button", { role: "menuitem", disabled: !p.ok, onclick: () => { closeMenu(); moveToStatus(i, s.name); } },
      h("span", { class: "dot", style: { "--c": `var(${CAT_VAR[s.category]})` } }), `Move to ${label(s.name)}`, h("small", {}, planText(p))));
  }
  menuEl = h("div", { class: "menu", role: "menu", "aria-label": `Move ${i.key}` }, items);
  document.body.append(menuEl);
  const r = anchor.getBoundingClientRect();
  const top = Math.min(r.bottom + 4, innerHeight - menuEl.offsetHeight - 8);
  const left = Math.min(r.left, innerWidth - menuEl.offsetWidth - 8);
  menuEl.style.top = `${Math.max(8, top)}px`;
  menuEl.style.left = `${Math.max(8, left)}px`;
  menuEl.querySelector("button:not(:disabled)")?.focus();
}
document.addEventListener("click", (e) => { if (menuEl && !menuEl.contains(e.target) && !e.target.closest("[data-menu]")) closeMenu(); });
document.addEventListener("keydown", (e) => { if (e.key === "Escape") { closeMenu(); closeDrawer(); } });
let toastT;
function toast(msg, err) {
  const t = $("toast");
  t.textContent = msg;
  t.className = `toast show${err ? " err" : ""}`;
  clearTimeout(toastT);
  toastT = setTimeout(() => (t.className = "toast"), err ? 5000 : 3200);
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
  const col = { epic: "#7A5AF8", story: "#2E9E5B", task: "#2F6FE4", bug: "#D64545", "sub-task": "#5C8DD6" }[t] || "#888";
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
function claimBar(i) {
  const cs = claimState(i);
  if (cs === "live") {
    const left = new Date(i.lease_until) - now();
    return h("div", { class: "claimbar live" }, `Claimed by ${i.assignee}, ${ago(left)} left`,
      h("span", { class: "leasebar", title: `Lease until ${clock(i.lease_until)}` }, h("i", { style: { width: `${Math.max(0, Math.min(100, left / LEASE_MS * 100))}%` } })));
  }
  if (cs === "stale") return h("div", { class: "claimbar stale" }, `Stale: ${i.assignee}'s lease expired ${ago(now() - new Date(i.lease_until))} ago`);
  return null;
}
// A Stale card's amber claim bar is its band, so it gets no purple one.
const showsHumanBand = (i) => needsHuman(i) && claimState(i) !== "stale";
const humanBar = (i) => (showsHumanBand(i) ? h("div", { class: "humanbar" }, needsHuman(i)) : null);
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
  if (compact && humanClass(i)) out.push(h("span", { class: "chip human", title: needsHuman(i) }, "Needs a human"));
  if (i.sub_issue_count) out.push(h("span", { class: "chip", title: "Sub-issues resolved" }, `${i.resolved_sub_issue_count}/${i.sub_issue_count} done`));
  if (i.parent && !compact) out.push(h("span", { class: "chip", title: "Parent" }, `in ${keyOf(i.parent)}`));
  if (i.eligible_for_close) out.push(h("span", { class: "chip live" }, "Ready to close"));
  if (readyForAgents(i) && !compact) out.push(h("span", { class: "chip live" }, "Ready"));
  return out;
}
function whoEl(i) {
  return h("span", { class: "who" }, ring(i), h("span", {}, i.assignee ? (i.assignee === S.viewer ? `${i.assignee} (you)` : i.assignee) : "Unassigned"));
}
function moreBtn(i) {
  return h("button", { class: "btn ghost more", "data-menu": "1", "aria-label": `Move ${i.key} to another column`, title: "Move to…",
    onclick: (e) => { e.stopPropagation(); moveMenu(i, e.currentTarget); } }, "⋯");
}
function staleButtons(i) {
  if (claimState(i) !== "stale") return null;
  const acts = i.stale_actions || [];
  return h("div", { class: "staleacts" }, acts.length
    ? acts.map((a) => h("button", { class: a.label === "Take over" ? "btn warn" : "btn", onclick: (e) => { e.stopPropagation(); runStale(i, a); } }, a.label))
    : h("span", { class: "key" }, "Release it from the CLI."));
}
const dotStyle = (cat) => ({ "--c": `var(${CAT_VAR[cat] || "--cat-todo"})` });

// ---------- detail (drawer in Columns, pane in Queue) ----------
function detailEl(i, extra) {
  const ck = i.checkpoint;
  const ckEl = ck && h("div", { class: "ckpt" },
    ck.progress && h("div", {}, h("b", {}, "Progress: "), ck.progress),
    ck.next && h("div", {}, h("b", {}, "Next: "), ck.next),
    ck.blocker && h("div", {}, h("b", {}, "Blocker: "), ck.blocker),
    ck.by && h("div", { class: "key" }, `${ck.by}${ck.at ? `, ${new Date(ck.at).toLocaleString()}` : ""}`));
  const moves = S.wf.statuses.filter((s) => s.name !== i.status).map((s) => {
    const p = i.menu?.[s.name] || { ok: false, reason: "" };
    return h("button", { disabled: !p.ok, onclick: () => moveToStatus(i, s.name) }, h("span", { class: "dot", style: dotStyle(s.category) }),
      `Move to ${label(s.name)}`, h("small", {}, planText(p)));
  });
  const subs = S.issues.filter((x) => x.parent === i.id);
  return h("div", { class: "detail" },
    h("div", { class: "row" }, typeGlyph(i.type), h("span", { class: "key" }, i.key), extra),
    h("h3", {}, i.title),
    claimBar(i), humanBar(i),
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
      h("dt", {}, "Updated"), h("dd", {}, new Date(i.updated_at).toLocaleString())),
    h("section", { class: "bodyslot", "data-body": i.id }, bodyEls(i)),
    linksEl(i),
    ckEl && h("section", {}, h("h4", {}, "Checkpoint"), ckEl),
    subs.length ? h("section", {}, h("h4", {}, `Sub-issues (${subs.length})`), h("div", { class: "moves" }, subs.map((s) =>
      h("button", { onclick: () => select(s.id) }, h("span", { class: "dot", style: dotStyle(catOf(s.status)) }), `${s.key} ${s.title}`, h("small", {}, label(s.status)))))) : null,
    h("section", {}, h("h4", {}, "Move"), h("div", { class: "moves" }, moves)));
}
function bodyEls(i) {
  const b = S.bodies.get(i.id);
  if (!b || b.version !== i.updated_at) return h("h4", {}, "Loading description…");
  const comments = b.comments || [];
  return [h("h4", {}, "Description"), h("pre", {}, b.body || "No description."),
    comments.length ? [h("h4", { class: "sub" }, `Comments (${comments.length})`), ...comments.slice(-3).map((c) => h("pre", {}, `${c.author}: ${c.body}`))] : null];
}
function linksEl(i) {
  const row = (x) => h("button", { onclick: () => select(x.id), onmouseenter: () => showEdges(x), onmouseleave: clearEdges },
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
const closeBtn = () => h("button", { class: "btn ghost push", onclick: closeDrawer, "aria-label": "Close" }, "✕");
function select(id) {
  S.selected = id;
  const i = S.byId.get(id);
  if (!i) return;
  if (layout() === "queue" && !narrow()) { render(); return; }
  const d = $("drawer");
  d.replaceChildren(detailEl(i, closeBtn()));
  d.classList.add("open");
  loadBody(i);
}
function closeDrawer() { $("drawer").classList.remove("open"); if (layout() !== "queue") S.selected = null; }

// ---------- filtering ----------
function visible() {
  const q = S.q.toLowerCase();
  return S.issues.filter((i) => {
    if (q && !`${i.key} ${i.title} ${(i.labels || []).join(" ")} ${i.assignee || ""}`.toLowerCase().includes(q)) return false;
    if (S.type && i.type !== S.type) return false;
    if (S.repo && (i.repository || "(board-wide)") !== S.repo) return false;
    if (S.who === "~none" && i.assignee) return false;
    if (S.who && S.who !== "~none" && (i.assignee || "") !== (S.who === "~me" ? S.viewer : S.who)) return false;
    if (S.epic && i.id !== S.epic && !ancestors(i).includes(S.epic)) return false;
    if (S.pulse && !PULSES[S.pulse].test(i)) return false;
    if (closedCat(catOf(i.status)) && !S.showAllClosed && now() - new Date(i.updated_at) > CLOSED_WINDOW_MS) return false;
    return true;
  });
}
// The header pills; one at a time filters the board.
const PULSES = {
  agents: { text: "ready for agents", test: readyForAgents },
  forhuman: { text: "ready for a human", test: readyForHuman },
  human: { text: "need a human", test: (i) => !!needsHuman(i), warn: true },
  active: { text: "in progress", test: (i) => catOf(i.status) === "active" },
  claimed: { text: "claimed", test: (i) => claimState(i) === "live" },
  stale: { text: "stale", test: (i) => claimState(i) === "stale", warn: true },
  close: { text: "ready to close", test: (i) => !!i.eligible_for_close },
};
const byPrio = (a, b) => S.prios.indexOf(a.priority) - S.prios.indexOf(b.priority) || (a.key < b.key ? -1 : 1);

// ---------- drag and drop (targets and reasons come from each Issue's `moves`) ----------
let dragId = null;
function dragStart(e, i) {
  dragId = i.id; e.dataTransfer.effectAllowed = "move"; e.dataTransfer.setData("text/plain", i.key);
  e.currentTarget.classList.add("dragging");
  closeMenu();
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
  const movable = !claimState(i) && i.column !== "blocked";
  return { draggable: movable ? "true" : "false", ondragstart: movable ? (e) => dragStart(e, i) : null, ondragend: movable ? dragEnd : null };
}
const stateClass = (i) => ({ live: "is-live", stale: "is-stale" })[claimState(i)] || "";

// ---------- Columns layout ----------
function renderColumns(stage, list) {
  const cols = columns().map((col) => {
    const items = list.filter((i) => i.column === col.id).sort(byPrio);
    const style = { "--c": col.color };
    if (col.id === "done" && !S.expanded.has("done")) {
      const show = () => { S.expanded.add("done"); render(); };
      return dropZone(h("section", { class: "col collapsed", style, title: "Show Done", tabindex: 0, role: "button", "aria-label": `Done, ${items.length} Issues, show`,
        onclick: show, onkeydown: (e) => { if (e.key === "Enter") show(); } },
        h("div", { class: "vlabel" }, `Done  ${items.length}`)), col.id);
    }
    const idle = col.id === "blocked" && !items.length ? "Nothing is blocked." : "";
    const zone = dropZone(h("section", { class: "col", style, "aria-label": col.name },
      h("header", {}, h("h2", {}, col.name), h("span", { class: "n" }, items.length),
        col.id === "done" ? h("button", { class: "btn ghost push", onclick: () => { S.expanded.delete("done"); render(); } }, "Hide") : null),
      h("p", { class: "why" }, idle),
      h("div", { class: "list" }, items.map((i) => {
        const cancelled = catOf(i.status) === "canceled";
        const chip = cancelled ? "Cancelled" : i.status !== col.home ? label(i.status) : null;
        const foot = badges(i, false, true);
        return h("article", { class: `card ${stateClass(i)} ${humanClass(i)} ${cancelled ? "cancelled" : ""}`, tabindex: 0, ...edgeProps(i), ...draggableProps(i),
          onclick: () => select(i.id), onkeydown: (e) => { if (e.target !== e.currentTarget) return; if (e.key === "Enter") select(i.id); if (e.key === "m") moveMenu(i, e.currentTarget); } },
          claimBar(i), humanBar(i),
          h("div", { class: "head" }, typeGlyph(i.type), h("span", { class: "key" }, i.key), prioGlyph(i.priority), chip ? h("span", { class: "chip status" }, chip) : null, moreBtn(i)),
          h("div", { class: "title" }, i.title),
          h("div", { class: "foot" }, whoEl(i)),
          foot.length ? h("div", { class: "foot" }, foot) : null,
          staleButtons(i));
      }))), col.id);
    zone.dataset.idle = idle;
    return zone;
  });
  stage.replaceChildren(h("div", { class: "cols" }, cols));
}

// ---------- Queue layout ----------
function renderQueue(stage, list) {
  const human = list.filter((i) => needsHuman(i));
  const rest = list.filter((i) => !needsHuman(i));
  const inCol = (id) => rest.filter((i) => i.column === id);
  const sections = [
    ["Needs a human", "Nothing moves until a person takes the step named on each Issue.", human.sort(byPrio)],
    ["In progress", "Being worked by agents or people.", rest.filter((i) => catOf(i.status) === "active").sort(byPrio)],
    ["Ready", "Agents take these in order; Issues marked For a human wait for a person.", inCol("ready").sort((a, b) => readyForHuman(a) - readyForHuman(b) || byPrio(a, b))],
    ["Blocked", "They move to Ready when their blockers close.", inCol("blocked").sort(byPrio)],
    ["Todo", "Not ready to start yet.", inCol("todo").sort(byPrio)],
    ["Closed recently", S.showAllClosed ? "All closed Issues." : "Closed in the last 14 days.", inCol("done").sort((a, b) => b.updated_at.localeCompare(a.updated_at))],
  ];
  const flat = sections.flatMap((s) => s[2]);
  if (!S.selected || !flat.some((i) => i.id === S.selected)) S.selected = flat[0]?.id || null;
  const rows = sections.filter((s) => s[2].length).map(([title, hint, xs]) => h("section", { class: "qsec" },
    h("h2", {}, title, h("span", { class: "n" }, xs.length)), h("p", { class: "hint" }, hint),
    xs.map((i) => h("div", { class: `qrow ${stateClass(i)} ${humanClass(i)}`, role: "option", tabindex: S.selected === i.id ? 0 : -1, "aria-selected": String(S.selected === i.id), ...edgeProps(i),
      onclick: () => select(i.id) },
      h("button", { class: "statusbtn", "data-menu": "1", style: dotStyle(catOf(i.status)), "aria-label": `${label(i.status)}, move`, onclick: (e) => { e.stopPropagation(); moveMenu(i, e.currentTarget); } }, h("span", { class: "dot" }), label(i.status)),
      h("span", { class: "row" }, prioGlyph(i.priority), h("span", { class: "key" }, i.key)),
      h("span", { class: "title", title: i.title }, typeGlyph(i.type), " ", i.title, " ", ...badges(i, true)),
      whoEl(i)))));
  const sel = S.byId.get(S.selected);
  stage.replaceChildren(h("div", { class: "queue" },
    h("div", { class: "qlist", role: "listbox", "aria-label": "Issues", onkeydown: qKeys }, rows.length ? rows : h("p", { class: "qempty" }, "No Issues match these filters.")),
    h("div", { class: "qdetail" }, sel ? detailEl(sel) : h("p", { class: "qempty" }, "Select an Issue."))));
  if (sel) loadBody(sel);
}
function qKeys(e) {
  const ids = [...document.querySelectorAll(".qrow")].map((r) => r.dataset.id);
  const at = ids.indexOf(S.selected);
  const go = (n) => { e.preventDefault(); S.selected = ids[Math.max(0, Math.min(ids.length - 1, at + n))]; render(); document.querySelector('.qrow[aria-selected="true"]')?.focus(); };
  if (e.key === "j" || e.key === "ArrowDown") go(1);
  if (e.key === "k" || e.key === "ArrowUp") go(-1);
  if (e.key === "Enter" && narrow() && S.selected) select(S.selected);
  if (e.key === "m") { const row = document.querySelector('.qrow[aria-selected="true"]'); if (row) moveMenu(S.byId.get(S.selected), row.querySelector(".statusbtn")); }
}

// ---------- chrome: pulse counts and filters ----------
function renderControls() {
  const all = S.issues;
  $("pulse").replaceChildren(...Object.entries(PULSES).map(([k, p]) => {
    const n = all.filter(p.test).length;
    return h("button", { class: p.warn && n ? "warn" : "", "aria-pressed": String(S.pulse === k), onclick: () => { S.pulse = S.pulse === k ? "" : k; renderControls(); render(); } }, h("b", {}, n), p.text);
  }));
  $("me").replaceChildren(ring({ assignee: S.viewer, assignee_kind: "human" }), `Acting as ${S.viewer}`);
  $("src").textContent = "Live board, refreshed every 3 s";
  // Rebuilding the filters while the viewer types would drop focus and the caret.
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
    h("select", { "aria-label": "Epic", onchange: (e) => { S.epic = e.target.value; render(); } }, opt("", "All epics", S.epic), epics.map((x) => opt(x.id, `${x.key} ${x.title.slice(0, 40)}`, S.epic))),
    h("label", {}, h("input", { type: "checkbox", checked: S.showAllClosed, onchange: (e) => { S.showAllClosed = e.target.checked; render(); } }), "Show all closed"));
}

function render(flashId) {
  if (!S.wf) return;
  const v = layout();
  $("layout").replaceChildren(...Object.entries(LAYOUTS).map(([k, name]) => h("button", { role: "radio", "aria-checked": String(k === v), onclick: () => setLayout(k) }, name)));
  const list = visible();
  const view = saveView();
  if (v === "queue") renderQueue($("stage"), list); else renderColumns($("stage"), list);
  const d = $("drawer");
  if (S.selected && d.classList.contains("open")) {
    const i = S.byId.get(S.selected);
    if (i) { d.replaceChildren(detailEl(i, closeBtn())); loadBody(i); } else closeDrawer();
  }
  restoreView(view);
  if (flashId) document.querySelector(`[data-id="${CSS.escape(flashId)}"]`)?.classList.add("flash");
}

// A re-render replaces the stage; keep the viewer's scroll positions and focused card or row.
const scrollKey = (el) => `${el.className}|${el.closest("[data-drop]")?.dataset.drop || el.querySelector("[data-body]")?.dataset.body || ""}`;
function saveView() {
  const a = document.activeElement;
  const focus = a && $("stage").contains(a) && a.matches(".card, .qrow") ? a.dataset.id : null;
  const scroll = new Map([...document.querySelectorAll("#stage .cols, #stage .list, #stage .qlist, #stage .qdetail")].map((el) => [scrollKey(el), [el.scrollLeft, el.scrollTop]]));
  return { focus, scroll };
}
function restoreView({ focus, scroll }) {
  for (const el of document.querySelectorAll("#stage .cols, #stage .list, #stage .qlist, #stage .qdetail")) {
    const at = scroll.get(scrollKey(el));
    if (at) [el.scrollLeft, el.scrollTop] = at;
  }
  if (focus) $("stage").querySelector(`.card[data-id="${CSS.escape(focus)}"], .qrow[data-id="${CSS.escape(focus)}"]`)?.focus({ preventScroll: true });
}

// ---------- layout switch ----------
function setLayout(name) {
  try { localStorage.setItem("orbit.layout", name); } catch {}
  const u = new URL(location.href); u.searchParams.set("layout", name); history.replaceState(null, "", u);
  closeDrawer(); closeMenu(); render();
}

// ---------- start ----------
if (!TOKEN) setError("This page has no token. Open the link that orbit board view printed (a reload drops the token).");
else refresh();
setInterval(poll, POLL_MS);
setInterval(() => { if (!busy()) render(); }, 30e3); // lease rings and "min left" tick between snapshot changes
