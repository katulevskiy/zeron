const app = document.querySelector("#app");
let state,
  selected = null,
  view = "queue",
  search = "",
  statusFilter = "",
  areaFilter = "";
let toastTimer;
let stateEtag = "";
const h = (value) =>
  String(value ?? "").replace(
    /[&<>"']/g,
    (char) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        char
      ],
  );
const human = (value) => String(value || "").replaceAll("-", " ");
const age = (date) =>
  Math.max(0, Math.floor((Date.now() - Date.parse(date)) / 86400000));
const shortActor = (actor) => actor?.replace("demo-", "") || "Unclaimed";
const has = (role) => state.roles.includes(role);
const tag = (value, tone = "") =>
  `<span class="tag ${tone}"><span class="dot"></span>${h(human(value))}</span>`;
const tone = (value) =>
  value === "ready-to-merge" || value === "merged"
    ? "green"
    : value === "changes-requested" || value === "needs-direction"
      ? "orange"
      : value === "needs-validation"
        ? "violet"
        : "";
const activeItems = () => state.items.filter((i) => !i.closed && !i.merged);
const needsMaintainer = (item) =>
  ["needs-triage", "needs-direction", "ready-to-merge"].includes(item.state) ||
  (item.risk === "high" && item.riskApproval !== item.revision);
function toast(message) {
  const node = document.querySelector("#toast");
  node.textContent = message;
  node.style.display = "block";
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    node.style.display = "none";
  }, 5000);
}
async function api(path, body) {
  const result = await fetch(path, {
    method: body ? "POST" : "GET",
    headers: body
      ? {
          "Content-Type": "application/json",
          "X-CSRF-Token": state?.csrf || "",
        }
      : path === "/api/state" && stateEtag
        ? { "If-None-Match": stateEtag }
        : {},
    body: body ? JSON.stringify(body) : undefined,
  });
  if (result.status === 304 && path === "/api/state") return state;
  if (path === "/api/state" && result.ok)
    stateEtag = result.headers.get("etag") || "";
  const data = await result.json();
  if (!result.ok) throw new Error(data.error || "Request failed.");
  return data;
}
async function refresh() {
  const previous = state;
  state = await api("/api/state");
  if (previous === state && !selected) return;
  if (selected) selected = await api(`/api/items/${selected.id}`);
  render();
}
function filtered() {
  let rows = activeItems().filter((i) => i.kind === "pr");
  if (view === "inbox") rows = rows.filter(needsMaintainer);
  if (view === "mine")
    rows = activeItems().filter(
      (i) =>
        i.author === state.actor ||
        i.claims.some((c) => c.actor === state.actor),
    );
  return rows
    .filter(
      (i) =>
        (!statusFilter || i.state === statusFilter) &&
        (!areaFilter || i.area === areaFilter) &&
        `${i.title} ${i.number} ${i.author} ${i.area}`
          .toLowerCase()
          .includes(search.toLowerCase()),
    )
    .sort(
      (a, b) =>
        ({ urgent: 0, normal: 1, low: 2 })[a.priority] -
          { urgent: 0, normal: 1, low: 2 }[b.priority] ||
        Date.parse(a.createdAt) - Date.parse(b.createdAt),
    );
}
function rowsHTML() {
  const rows = filtered();
  return (
    `<div class="queue-head"><span>Change / next action</span><span>Review stage</span><span>Active work</span><span>Age / decision</span></div>` +
    (rows.length
      ? rows
          .map(
            (item) => `<button class="queue-row" data-open="${h(item.id)}">
      <div><div class="item-title">${h(item.title)}</div><div class="item-meta"><span class="item-number">#${item.number}</span><span>${h(item.area)}</span><span>${h(item.risk)} risk</span>${item.priority === "urgent" ? '<span class="missing">Urgent</span>' : ""}</div>
      <div class="item-meta">${h(item.blockers[0] || "Review evidence complete")}</div></div>
      <div>${tag(item.state, tone(item.state))}</div>
      <div class="owner">${item.claims.length ? `<strong>${h(shortActor(item.claims[0].actor))}</strong>${h(item.claims[0].task.replace("validate:", "Test "))}${item.claims.length > 1 ? ` +${item.claims.length - 1}` : ""}` : item.state === "ready-to-merge" ? "<strong>Awaiting merger</strong>Final decision" : "<strong>Available</strong>Pick up a task"}</div>
      <div class="age">${age(item.createdAt)} days open<br><span class="${Date.parse(item.decisionDue) < Date.now() ? "late" : ""}">${Date.parse(item.decisionDue) < Date.now() ? "Decision overdue" : `${Math.ceil((Date.parse(item.decisionDue) - Date.now()) / 86400000)}d to decision`}</span></div>
    </button>`,
          )
          .join("")
      : '<div class="empty">No items in this view.<br>Try another filter or pick work from the review queue.</div>')
  );
}
function queueHTML() {
  const areas = [...new Set(activeItems().map((i) => i.area))].sort();
  const stages = [...new Set(activeItems().map((i) => i.state))].sort();
  return `<div class="filters"><div class="search"><input id="search" aria-label="Search changes" placeholder="Search changes, contributors, or PR numbers…" value="${h(search)}"></div>
    <select id="status-filter" aria-label="Filter by review stage"><option value="">All stages</option>${stages.map((s) => `<option value="${h(s)}" ${statusFilter === s ? "selected" : ""}>${h(human(s))}</option>`).join("")}</select>
    <select id="area-filter" aria-label="Filter by area"><option value="">All areas</option>${areas.map((a) => `<option ${areaFilter === a ? "selected" : ""}>${h(a)}</option>`).join("")}</select><span class="results">${filtered().length} changes</span></div>
    <div class="queue" id="queue-table">${rowsHTML()}</div><p class="note">Readiness comes from revision-specific evidence. Demand and urgency inform priority; they do not replace review.</p>`;
}
function roadmapHTML() {
  const issues = activeItems().filter((i) => i.kind === "issue");
  const card = (item) =>
    `<button class="roadmap-card" data-open="${h(item.id)}"><div class="item-title">${h(item.title)}</div><div class="item-meta"><span>#${item.number}</span><span>${h(item.area)}</span></div><div class="item-meta">${tag(item.priority === "urgent" ? "urgent" : item.risk + " risk", item.priority === "urgent" ? "orange" : "")}</div></button>`;
  return `<div class="roadmap">${["now", "next", "later"]
    .map(
      (plan) =>
        `<section class="roadmap-col"><h3>${human(plan)} <span class="count">${issues.filter((i) => i.plan === plan).length}</span></h3>${
          issues
            .filter((i) => i.plan === plan)
            .map(card)
            .join("") || '<p class="note">No planned items yet.</p>'
        }</section>`,
    )
    .join("")}</div>
    ${
      issues.some((i) => i.plan === "needs-triage")
        ? `<div class="panel"><h3>Needs triage</h3>${issues
            .filter((i) => i.plan === "needs-triage")
            .map(card)
            .join("")}</div>`
        : ""
    }
    <p class="note">Issues describe accepted problems and plans. PRs carry implementation and review evidence. Open an issue to place it on the roadmap.</p>`;
}
function peopleHTML() {
  const actors = new Set([
    ...state.demoActors,
    ...state.items.flatMap((i) => [
      i.author,
      ...i.reviews.map((r) => r.actor),
      ...i.validations.map((v) => v.actor),
    ]),
  ]);
  return `<div class="people">${[...actors]
    .sort()
    .map((actor) => {
      const reviews = state.items
        .flatMap((i) => i.reviews)
        .filter((r) => r.actor === actor);
      const validations = state.items
        .flatMap((i) => i.validations)
        .filter((v) => v.actor === actor);
      const authored = state.items.filter((i) => i.author === actor).length;
      const platforms = [...new Set(validations.map((v) => v.platform))];
      return `<article class="person"><div class="avatar">${h(shortActor(actor).slice(0, 2).toUpperCase())}</div><h3>${h(actor)}</h3><p>${actor === state.actor && state.roles.length ? h(state.roles.join(" · ")) : "Contributor"}</p>
      <div class="contributions">${authored} authored items<br>${reviews.length} review records<br>${validations.length} validation records</div><p>${platforms.length ? `Validation experience: ${h(platforms.join(", "))}` : "Expertise grows through inspectable contribution evidence."}</p></article>`;
    })
    .join(
      "",
    )}</div><p class="note">These counts describe activity. Capability grants are explicit maintainer decisions; activity does not automatically grant privileges.</p>`;
}
function render() {
  document.body.classList.toggle("detail-open", Boolean(selected));
  const items = activeItems();
  const prs = items.filter((i) => i.kind === "pr");
  const ready = prs.filter((i) => i.state === "ready-to-merge").length;
  const claimed = prs.filter((i) => i.claims.length).length;
  const direction = prs.filter((i) =>
    ["needs-triage", "needs-direction"].includes(i.state),
  ).length;
  const titles = {
    queue: [
      "Review queue",
      "Every change, its next action, and the evidence behind it.",
    ],
    inbox: [
      "Maintainer inbox",
      "Direction decisions, risk approvals, and changes ready for a final decision.",
    ],
    mine: [
      "My work",
      "Your active claims and authored changes, together in one place.",
    ],
    roadmap: [
      "Bugs & roadmap",
      "A shared view of the problems we are solving and what comes next.",
    ],
    people: [
      "Contributors",
      "Recognize implementation, review, and validation as valuable work.",
    ],
  };
  const nav = [
    ["queue", "▤", "Review queue", prs.length],
    ["inbox", "◇", "Maintainer inbox", prs.filter(needsMaintainer).length],
    [
      "mine",
      "◷",
      "My work",
      items.filter((i) => i.claims.some((c) => c.actor === state.actor)).length,
    ],
    [
      "roadmap",
      "↗",
      "Bugs & roadmap",
      items.filter((i) => i.kind === "issue").length,
    ],
    ["people", "○", "Contributors", null],
  ];
  app.innerHTML = `<div class="shell"><aside class="sidebar"><div class="brand"><img class="mark" src="/assets/zeron-favicon-v3.png" alt=""><div>Zeron<small>Review workspace</small></div></div>
    <nav class="nav" aria-label="Workspace">${nav.map(([id, icon, title, count]) => `<button class="${view === id ? "active" : ""}" data-view="${id}"><span class="nav-icon">${icon}</span>${title}${count !== null ? `<span class="count">${count}</span>` : ""}</button>`).join("")}</nav>
    <div class="side-footer"><strong>Shared work. Visible progress.</strong><br>Every change has a next action.<br><a href="https://github.com/${h(state.repository)}" target="_blank" rel="noreferrer">${h(state.repository)} ↗</a></div></aside>
    <div class="workspace"><header class="topbar"><div class="breadcrumb"><span>${h(state.repository)}</span><span>/</span>${h(titles[view][0])}</div>
    <div class="user">${state.mode === "demo" ? `<select id="demo-actor" aria-label="Demo identity"><option value="">Try a contributor role</option>${state.demoActors.map((actor) => `<option value="${actor}" ${state.actor === actor ? "selected" : ""}>${h(shortActor(actor))}</option>`).join("")}</select>` : state.actor ? `<span>${h(state.actor)}</span><button class="quiet" data-logout>Sign out</button>` : state.loginEnabled ? '<a href="/auth/github">Sign in with GitHub</a>' : "<span>Read-only preview</span>"}<span class="avatar">${h(shortActor(state.actor).slice(0, 2).toUpperCase())}</span></div></header>
    ${state.mode === "demo" ? '<div class="banner"><span><strong>Sandbox demo</strong> · All decisions and claims stay here. Sample scenarios are illustrative.</span><span>Choose a role to try the workflow</span></div>' : !state.writeback ? '<div class="banner"><span><strong>GitHub writeback is off</strong> · Workspace actions are stored locally.</span></div>' : ""}
    <main class="content"><div class="heading"><div><div class="eyebrow">Contribution workflow</div><h1>${h(titles[view][0])}</h1><p>${h(titles[view][1])}</p></div><div>${state.mode !== "demo" && has("maintainer") ? "<button data-sync>Sync GitHub</button>" : '<a href="https://github.com/' + h(state.repository) + '/pulls" target="_blank" rel="noreferrer">View upstream ↗</a>'}<div class="subtle">${state.lastReconciled ? `Last reconciled ${h(new Date(state.lastReconciled).toLocaleTimeString())}` : state.mode === "demo" ? "Demo workspace · persistent local state" : "Waiting for first reconciliation"}</div></div></div>
    ${state.syncError || state.writebackError ? `<div class="error-box">${h(state.syncError?.message || state.writebackError?.message)}. The worker will retry; existing evidence remains available.</div>` : ""}
    <div class="stats"><div class="stat"><div class="label">Open changes</div><div class="number">${prs.length}</div><div class="hint">Across all review stages</div></div><div class="stat"><div class="label">Direction decisions</div><div class="number">${direction}</div><div class="hint">Clarify scope before more work</div></div><div class="stat"><div class="label">Active reviews</div><div class="number">${claimed}</div><div class="hint">Work claimed by contributors</div></div><div class="stat green"><div class="label">Ready to merge</div><div class="number">${ready}</div><div class="hint">Required evidence complete</div></div></div>
    ${view === "roadmap" ? roadmapHTML() : view === "people" ? peopleHTML() : queueHTML()}
    <div class="footerline"><span>Direction → Review → Validation → Merge decision</span><span>${state.policy.decisionDays}-day disposition target · ${state.policy.claimHours}-hour task claims</span></div></main></div></div>${selected ? detailHTML(selected) : ""}`;
  if (selected) document.querySelector(".drawer-top button")?.focus();
}
function field(label, name, type = "text", value = "") {
  return `<label class="form-field">${h(label)}${type === "textarea" ? `<textarea name="${name}" required>${h(value)}</textarea>` : `<input name="${name}" type="${type}" value="${h(value)}" required>`}</label>`;
}
function selectField(label, name, options, value) {
  return `<label class="form-field">${h(label)}<select name="${name}">${options.map((o) => `<option value="${h(o)}" ${o === value ? "selected" : ""}>${h(human(o))}</option>`).join("")}</select></label>`;
}
function detailHTML(item) {
  const review = item.reviews.filter((r) => r.revision === item.revision);
  const hasReview = review.some(
    (r) => r.verdict === "pass" && r.actor !== item.author,
  );
  const tasks =
    item.kind === "pr"
      ? ["code-review", ...item.platforms.map((p) => `validate:${p}`)]
      : ["reproduce"];
  const independent = state.actor && state.actor !== item.author;
  const evidence = [
    ...item.reviews.map((r) => ({ ...r, title: `Code review · ${r.verdict}` })),
    ...item.validations.map((v) => ({
      ...v,
      title: `${v.platform} validation · ${v.verdict}`,
    })),
  ].sort((a, b) => Date.parse(b.at) - Date.parse(a.at));
  const open = !item.closed && !item.merged;
  return `<div class="overlay" data-overlay><section class="drawer" role="dialog" aria-modal="true" aria-labelledby="detail-title"><div class="drawer-top"><span class="eyebrow">${item.kind === "pr" ? "Pull request" : "Issue"} · #${item.number}${item.sample ? " · SAMPLE" : ""}</span><div class="user">${state.mode === "demo" ? `<select id="detail-demo-actor" aria-label="Demo identity in detail"><option value="">Try a role</option>${state.demoActors.map((actor) => `<option value="${actor}" ${state.actor === actor ? "selected" : ""}>${h(shortActor(actor))}</option>`).join("")}</select>` : ""}<button class="quiet" data-close aria-label="Close detail">✕</button></div></div>
    ${tag(item.state, tone(item.state))}<h2 id="detail-title">${h(item.title)}</h2><div class="description">Authored by ${h(item.author)} · ${age(item.createdAt)} days open${item.url ? ` · <a href="${h(item.url)}" target="_blank" rel="noreferrer">Open on GitHub ↗</a>` : ""}</div>
    <div class="detail-meta">${tag(item.area)}${tag(item.risk + " risk", item.risk === "high" ? "orange" : "")}<span class="mono description">${h(item.revision.slice(0, 8))}</span></div>
    ${item.sample ? '<p class="note">This is an illustrative scenario, not an upstream review or endorsement.</p>' : `<p class="note">Source last synchronized ${h(item.syncedAt || "unknown")}. Evidence and actions apply to the revision shown above.</p>`}
    <div class="panel"><h3>Next actions <span class="description">${item.blockers.length} remaining</span></h3>${item.blockers.map((b) => `<div class="check-row"><span>${h(b)}</span><span class="missing">○</span></div>`).join("") || (item.kind === "pr" ? '<p class="pass">Review evidence is complete. An authorized contributor makes the final merge decision on GitHub.</p>' : "")}
      ${
        item.kind === "pr"
          ? `<div class="check-row"><span>Code review on this revision</span><span class="${hasReview ? "pass" : "missing"}">${hasReview ? "Recorded" : "Needed"}</span></div>${state.policy.requiredChecks
              .map((name) => {
                const check = item.checks.find((c) => c.name === name);
                return `<div class="check-row"><span>${h(name)}</span><span class="${check?.conclusion === "success" ? "pass" : "missing"}">${h(check?.conclusion || "missing")}</span></div>`;
              })
              .join("")}`
          : `<p>Roadmap placement: ${h(human(item.plan))}. Claim reproduction or link an implementation on GitHub.</p>`
      }</div>
    <div class="panel"><h3>Coordinate the work</h3>${tasks
      .map((task) => {
        const claim = item.claims.find((c) => c.task === task);
        return `<div class="task"><div>${h(human(task.replace("validate:", "Validate ")))}<small>${claim ? `${h(claim.actor)} · expires ${h(new Date(claim.expires).toLocaleString())}` : "Available to claim"}</small></div><div class="buttons">${claim?.actor === state.actor ? `<button data-task="${h(task)}" data-task-action="renew">Renew</button><button data-task="${h(task)}" data-task-action="release">Release</button>` : `<button data-task="${h(task)}" data-task-action="claim" ${!(state.actor && (independent || item.kind === "issue")) || claim || !open ? "disabled" : ""}>Claim task</button>`}</div></div>`;
      })
      .join(
        "",
      )}${!state.actor ? "<p>Sign in or choose a demo identity to claim work.</p>" : ""}</div>
    ${has("triager") && open ? `<div class="panel"><details><summary>Triage scope, priority, and coverage</summary><form data-action="triage"><div class="form-grid">${field("Area", "area", "text", item.area)}${selectField("Risk", "risk", ["low", "medium", "high"], item.risk)}${selectField("Priority", "priority", ["urgent", "normal", "low"], item.priority)}${item.kind === "issue" ? selectField("Roadmap", "plan", ["needs-triage", "now", "next", "later"], item.plan) : ""}</div><div class="description">Required platform validation</div><div class="check-options">${["linux", "macos", "windows", "ios"].map((p) => `<label><input type="checkbox" name="platforms" value="${p}" ${item.platforms.includes(p) ? "checked" : ""}>${p}</label>`).join("")}</div><label class="form-field">Blocking dependency or concern<input name="blocker" value="${h(item.blocker || "")}"></label><button class="primary">Save triage</button></form></details></div>` : ""}
    ${item.kind === "pr" && has("triager") && open ? `<div class="panel"><details><summary>Record a direction decision</summary><form data-action="direction">${selectField("Decision", "verdict", ["accepted", "deferred", "rejected", "needs-decision"], item.direction)}${field("Accepted scope or decision reason", "reason", "textarea", item.directionReason || "")}<button class="primary">Record decision</button><p>${has("maintainer") ? "This decision establishes scope. Implementation still requires review." : "Triagers can accept low-risk work; other decisions need a maintainer."}</p></form></details></div>` : ""}
    ${item.kind === "pr" && has("reviewer") && independent && open ? `<div class="panel"><details><summary>Submit a code review</summary><form data-action="review">${selectField("Verdict", "verdict", ["pass", "changes"], "pass")}${field("Findings and scope reviewed", "summary", "textarea")}<button class="primary">Submit review</button></form></details></div>` : ""}
    ${has("validator") && independent && item.platforms.length && open ? `<div class="panel"><details><summary>Record behavior validation</summary><form data-action="validate"><div class="form-grid">${selectField("Platform", "platform", item.platforms, item.platforms[0])}${selectField("Verdict", "verdict", ["pass", "fail"], "pass")}</div>${field("Environment and build", "environment")}${field("Reproduction, steps exercised, and results", "summary", "textarea")}<label class="form-field">Build, log, or screenshot URL (optional)<input name="artifact" type="url" placeholder="https://…"></label><button class="primary">Record validation</button></form></details></div>` : ""}
    ${item.kind === "pr" && has("maintainer") && independent && item.risk === "high" && item.riskApproval !== item.revision && open ? `<div class="panel"><details><summary>Approve the risk of this revision</summary><form data-action="risk-approval">${field("Risk assessment and approval reason", "reason", "textarea")}<button class="primary">Approve risk</button></form></details></div>` : ""}
    ${
      state.actor && open
        ? `<div class="panel"><details><summary>Explain demand or urgency</summary><form data-action="vote">${selectField("Signal", "dimension", ["demand", "urgency"], "demand")}${field("Your use case or time-sensitive impact", "reason", "textarea")}<button>Record signal</button></form></details><p>${Object.values(item.votes || {}).filter((v) => v.dimension === "demand").length} demand signals · ${Object.values(item.votes || {}).filter((v) => v.dimension === "urgency").length} urgency signals</p>${Object.values(
            item.votes || {},
          )
            .map(
              (v) =>
                `<div class="evidence"><strong>${h(v.actor)} · ${h(v.dimension)}</strong><p>${h(v.reason)}</p></div>`,
            )
            .join("")}</div>`
        : ""
    }
    <div class="panel"><h3>Review & validation evidence</h3>${evidence.length ? evidence.map((record) => `<div class="evidence ${record.revision !== item.revision ? "stale" : ""}"><strong>${h(record.title)}</strong><p>${h(record.summary)}</p>${record.environment ? `<p>${h(record.environment)}</p>` : ""}${record.artifact ? `<a href="${h(record.artifact)}" target="_blank" rel="noreferrer">View artifact ↗</a>` : ""}<div class="by">${h(record.actor)} · ${h(record.revision.slice(0, 8))}${record.revision !== item.revision ? " · Previous revision; does not count" : ""}</div></div>`).join("") : "<p>No review evidence recorded yet.</p>"}</div>
    <div class="panel"><h3>Activity</h3><ul class="timeline">${item.events.map((event) => `<li><strong>${h(event.actor)}</strong> ${h(human(event.action))}<time>${h(new Date(event.at).toLocaleString())}</time></li>`).join("") || "<li>No workspace activity yet.</li>"}</ul></div>
    <p class="note">Native GitHub branch protections and integration checks still apply. This workspace never merges PRs automatically.</p></section></div>`;
}
function closeDetail() {
  selected = null;
  const url = new URL(location.href);
  url.searchParams.delete("item");
  history.replaceState({}, "", url);
  render();
}
document.addEventListener("click", async (event) => {
  const target = event.target.closest("button, [data-overlay]");
  if (!target) return;
  try {
    if (target.dataset.view) {
      view = target.dataset.view;
      statusFilter = "";
      areaFilter = "";
      search = "";
      render();
    } else if (target.dataset.open) {
      selected = await api(`/api/items/${target.dataset.open}`);
      const url = new URL(location.href);
      url.searchParams.set("item", selected.id);
      history.replaceState({}, "", url);
      render();
    } else if (
      target.hasAttribute("data-close") ||
      event.target === document.querySelector("[data-overlay]")
    )
      closeDetail();
    else if (target.dataset.taskAction) {
      target.disabled = true;
      await api(`/api/items/${selected.id}/${target.dataset.taskAction}`, {
        revision: selected.revision,
        task: target.dataset.task,
      });
      await refresh();
      toast("Task updated.");
    } else if (target.hasAttribute("data-sync")) {
      await api("/api/sync", {});
      toast("GitHub synchronization queued.");
    } else if (target.hasAttribute("data-logout")) {
      await api("/api/logout", {});
      await refresh();
    }
  } catch (error) {
    target.disabled = false;
    toast(error.message);
  }
});
document.addEventListener("change", async (event) => {
  if (
    ["demo-actor", "detail-demo-actor"].includes(event.target.id) &&
    event.target.value
  ) {
    try {
      await api("/api/demo/login", { actor: event.target.value });
      await refresh();
      toast(`Acting as ${shortActor(state.actor)} in the sandbox.`);
    } catch (error) {
      toast(error.message);
    }
  } else if (event.target.id === "status-filter") {
    statusFilter = event.target.value;
    document.querySelector("#queue-table").innerHTML = rowsHTML();
    document.querySelector(".results").textContent =
      `${filtered().length} changes`;
  } else if (event.target.id === "area-filter") {
    areaFilter = event.target.value;
    document.querySelector("#queue-table").innerHTML = rowsHTML();
    document.querySelector(".results").textContent =
      `${filtered().length} changes`;
  }
});
document.addEventListener("input", (event) => {
  if (event.target.id !== "search") return;
  search = event.target.value;
  document.querySelector("#queue-table").innerHTML = rowsHTML();
  document.querySelector(".results").textContent =
    `${filtered().length} changes`;
});
document.addEventListener("submit", async (event) => {
  const form = event.target;
  if (!form.dataset.action) return;
  event.preventDefault();
  const data = new FormData(form);
  const body = Object.fromEntries(data);
  body.revision = selected.revision;
  if (form.dataset.action === "triage")
    body.platforms = data.getAll("platforms");
  const button = form.querySelector("button");
  button.disabled = true;
  try {
    await api(`/api/items/${selected.id}/${form.dataset.action}`, body);
    await refresh();
    toast("Recorded for this revision.");
  } catch (error) {
    button.disabled = false;
    toast(error.message);
  }
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && selected) closeDetail();
  if (selected && event.key === "Tab") {
    const nodes = [
      ...document.querySelectorAll(
        ".drawer button:not(:disabled), .drawer a, .drawer input, .drawer textarea, .drawer select, .drawer summary",
      ),
    ].filter((el) => el.getClientRects().length);
    const first = nodes[0],
      last = nodes.at(-1);
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last?.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first?.focus();
    }
  }
});
try {
  state = await api("/api/state");
  const id = new URL(location.href).searchParams.get("item");
  if (id && /^(pr|issue):\d+$/.test(id))
    selected = await api(`/api/items/${id}`);
  render();
} catch (error) {
  app.innerHTML = `<main class="loading">${h(error.message)} <button data-retry>Retry</button></main>`;
  document
    .querySelector("[data-retry]")
    ?.addEventListener("click", () => location.reload());
}
setInterval(async () => {
  if (
    selected ||
    ["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement?.tagName)
  )
    return;
  try {
    await refresh();
  } catch {
    /* Keep the current view during a transient network failure. */
  }
}, 30000);
