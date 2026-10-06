const app = document.querySelector("#app");
let state,
  selected,
  view = "overview",
  query = "",
  stage = "all",
  layout = localStorage.getItem("cm-layout") || "list";
let modal = null,
  dragItem = null,
  busy = false;
const h = (value) =>
  String(value ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
const label = (value) =>
  String(value || "")
    .replaceAll("-", " ")
    .replaceAll("validate:", "Test ");
const safe = (value) => {
  try {
    const u = new URL(value);
    return u.protocol === "https:" ? h(u.href) : "#";
  } catch {
    return "#";
  }
};
const link = (url, text) =>
  url
    ? `<a href="${safe(url)}" target="_blank" rel="noopener noreferrer">${h(text)} ↗</a>`
    : "";
const can = (role) => state.roles.includes(role);
const active = (item) => !item.closed && !item.merged;
const prs = () => state.items.filter((i) => i.kind === "pr");
const short = (sha) => String(sha || "").slice(0, 8);
const stars = () =>
  state.actor
    ? state.stars || []
    : JSON.parse(localStorage.getItem("cm-stars") || "[]");
const isStar = (id) => stars().includes(id);
const badge = (text, tone = "") =>
  `<span class="badge ${tone}">${h(label(text))}</span>`;
const tone = (state) =>
  ["ready-to-merge", "merged", "success", "pass", "accepted"].includes(state)
    ? "green"
    : ["changes-requested", "failure", "fail", "error", "blocked"].includes(
          state,
        )
      ? "red"
      : "purple";
const time = (at) => (at ? new Date(at).toLocaleString() : "Not synced yet");
const roleName = () =>
  state.preview
    ? `Preview · ${state.actor?.replace("preview-", "") || "guest"}`
    : state.actor
      ? `@${state.actor} · ${state.access}`
      : "Guest";
async function api(path, body) {
  const response = await fetch(path, {
    method: body === undefined ? "GET" : "POST",
    headers:
      body === undefined
        ? {}
        : {
            "Content-Type": "application/json",
            "X-CSRF-Token": state?.csrf || "",
          },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const data = await response.json();
  if (!response.ok)
    throw new Error(
      data.error || data.message || `Request failed (${response.status})`,
    );
  return data;
}
function toast(message) {
  const node = document.querySelector("#toast");
  node.textContent = message;
  node.classList.add("show");
  setTimeout(() => node.classList.remove("show"), 4500);
}
async function refresh() {
  state = await api("/api/state");
  if (selected) selected = await api(`/api/items/${selected.id}`);
  render();
}
function nav() {
  return `<aside class="sidebar"><a class="brand" href="/"><img class="mark" src="/assets/zeron-favicon-v3.png" alt=""><span>Contribution<br><strong>Manager</strong></span></a><div class="repo-label">${link(`https://github.com/${state.repository}`, state.repository)}</div><nav aria-label="Workspace">${[
    ["overview", "Overview", "◈"],
    ["prs", "Pull requests", "↗"],
    ["bugs", "Bugs", "◉"],
    ["features", "Features", "✧"],
    ["mine", "My work", "✓"],
    ["stars", "Starred", "☆"],
  ]
    .map(
      ([key, name, icon]) =>
        `<button aria-label="${h(name)}" data-view="${key}" class="nav-item ${view === key && !selected ? "active" : ""}"><span aria-hidden="true">${icon}</span>${name}${key === "stars" ? `<span class="count">${stars().length}</span>` : ""}</button>`,
    )
    .join(
      "",
    )}</nav><div class="sidebar-bottom"><p>One shared queue.<br>A clear next step.</p><button data-report="bug">Report a bug</button><button data-report="feature">Propose a feature</button></div></aside>`;
}
function header() {
  return `<header class="topbar"><span class="identity">${badge(roleName(), state.preview ? "amber" : "")} ${state.roles.length ? `<span class="access-hint">${h(state.roles.join(" · "))}</span>` : ""}</span><div class="top-actions">${state.devPreview || state.mode === "demo" ? "<button data-preview>Try a role</button>" : ""}${state.actor || state.preview ? "<button data-logout>Sign out</button>" : state.loginEnabled ? '<a class="button primary" href="/auth/github">Sign in with GitHub</a>' : '<button disabled title="The repository owner must connect the GitHub App first">GitHub sign-in pending</button>'}</div></header>${state.preview ? '<div class="preview-banner"><strong>Sandbox preview</strong> · You are trying a role. Changes here never update the live repository. <button data-live>Return to live view</button></div>' : ""}${state.mode === "demo" ? '<div class="preview-banner">Local test sandbox · illustrative fixtures</div>' : ""}`;
}
function row(item, card = false) {
  const owned = item.claims
    ?.map((c) => `@${c.actor} · ${label(c.task)}`)
    .join(", ");
  const next =
    item.blockers?.[0] ||
    (item.kind === "pr" ? "Review packet complete" : "Ready for planning");
  return `<article class="pr-card ${card ? "board-card" : ""}" ${card ? 'draggable="true"' : ""} data-drag-id="${h(item.id)}"><button class="star ${isStar(item.id) ? "on" : ""}" data-star="${h(item.id)}" aria-label="${isStar(item.id) ? "Unstar" : "Star"} ${h(item.title)}">${isStar(item.id) ? "★" : "☆"}</button><button class="card-open" data-open="${h(item.id)}"><div class="row-meta"><span>${item.local ? "Report" : "#"}${h(item.number)}</span><span>@${h(item.upstream?.author || item.author)}</span>${item.priority === "urgent" ? badge("urgent", "amber") : ""}${item.upstreamUrl ? '<span class="source-label">Upstream mirror</span>' : ""}</div><h3>${h(item.title)}</h3><p class="next-line">${h(next)}</p><div class="row-footer">${badge(item.state, tone(item.state))}<span>${h(owned || "Unclaimed")}</span>${item.mergeable === false ? badge("conflicts", "red") : ""}</div></button></article>`;
}
function filtered() {
  let items =
    view === "bugs" || view === "features"
      ? state.items.filter(
          (i) =>
            i.kind === "issue" &&
            (view === "features"
              ? i.reportType === "feature" ||
                i.labels?.some((l) => /feature|enhancement/i.test(l))
              : i.reportType !== "feature" &&
                !i.labels?.some((l) => /feature|enhancement/i.test(l))),
        )
      : view === "stars"
        ? state.items.filter((i) => isStar(i.id))
        : view === "mine"
          ? state.items.filter(
              (i) =>
                i.author === state.actor ||
                i.claims?.some((c) => c.actor === state.actor),
            )
          : prs();
  if (stage !== "resolved") items = items.filter(active);
  else items = items.filter((i) => !active(i));
  if (!["all", "resolved"].includes(stage))
    items = items.filter((i) => i.state === stage);
  return items
    .filter((i) =>
      `${i.title} ${i.number} ${i.author} ${i.area} ${i.upstreamNumber || ""}`
        .toLowerCase()
        .includes(query.toLowerCase()),
    )
    .sort(
      (a, b) =>
        (b.priority === "urgent") - (a.priority === "urgent") ||
        Date.parse(b.updatedAt) - Date.parse(a.updatedAt),
    );
}
const stages = [
  "needs-triage",
  "needs-direction",
  "ready-for-review",
  "changes-requested",
  "needs-validation",
  "ready-to-merge",
];
function listView() {
  const titles = {
    prs: "Pull requests",
    bugs: "Bugs",
    features: "Features",
    mine: "My work",
    stars: "Starred",
  };
  const rows = filtered();
  const board = layout === "board" && view === "prs";
  return `<div class="page-title"><div><p class="eyebrow">${view === "stars" ? "YOUR WATCHLIST" : "THE SHARED WORKSPACE"}</p><h1>${titles[view]}</h1><p>${view === "stars" ? "Follow the changes you care about, from review to resolution." : view === "mine" ? "Your authored changes and the work you have claimed." : "Choose a change. See its next step. Move it forward."}</p></div><div>${view === "bugs" ? '<button class="primary" data-report="bug">Report a bug</button>' : view === "features" ? '<button class="primary" data-report="feature">Propose a feature</button>' : ""}</div></div><div class="toolbar"><input aria-label="Search work" id="search" placeholder="Search title, author or PR number" value="${h(query)}"><div class="segmented"><button data-layout="list" class="${!board ? "active" : ""}">List</button>${view === "prs" ? `<button data-layout="board" class="${board ? "active" : ""}">Board</button>` : ""}</div><span class="muted">${rows.length} items</span></div><div class="stage-filters" aria-label="Filter stages">${["all", ...stages, "resolved"].map((s) => `<button data-stage="${s}" class="${stage === s ? "active" : ""}">${h(label(s))}</button>`).join("")}</div>${
    board
      ? `<div class="board">${stages
          .map(
            (s) =>
              `<section class="board-column" data-drop-stage="${s}"><h2>${h(label(s))}<span>${rows.filter((i) => i.state === s).length}</span></h2><p class="column-help">${h({ "needs-triage": "Define scope and coverage", "needs-direction": "Accept, defer or decline", "ready-for-review": "Claim and review the code", "changes-requested": "Author addresses the findings", "needs-validation": "Test the current revision", "ready-to-merge": "Inspect the packet on GitHub" }[s])}</p>${
                rows
                  .filter((i) => i.state === s)
                  .map((i) => row(i, true))
                  .join("") || '<div class="empty-column">No work here</div>'
              }</section>`,
          )
          .join(
            "",
          )}</div><p class="note">Drag a PR to open the action needed for that stage. Readiness is computed from evidence.</p>`
      : `<div class="list">${rows.map((i) => row(i)).join("") || '<div class="empty">No matching work.<br>Try another stage or search.</div>'}</div>`
  }`;
}
function overview() {
  const open = prs().filter(active),
    ready = open.filter((i) => i.state === "ready-to-merge"),
    review = open.filter((i) => i.state === "ready-for-review"),
    urgent = open.filter((i) => i.priority === "urgent");
  const modules = {
    attention: {
      title: "Move the next change forward",
      hint: "Start here. Each PR tells you what is missing.",
      items: open
        .filter((i) =>
          can("maintainer")
            ? ["needs-triage", "needs-direction", "ready-to-merge"].includes(
                i.state,
              )
            : i.state === "ready-for-review",
        )
        .slice(0, 5),
    },
    starred: {
      title: "Following",
      hint: "Your starred PRs and their latest progress.",
      items: state.items.filter((i) => isStar(i.id)).slice(0, 5),
    },
    claims: {
      title: "Work in progress",
      hint: "Claims show who is reviewing or testing.",
      items: open.filter((i) => i.claims?.length).slice(0, 5),
    },
  };
  let order;
  try {
    order = JSON.parse(localStorage.getItem("cm-panels") || "[]");
  } catch {
    order = [];
  }
  order = [...new Set([...order, ...Object.keys(modules)])].filter(
    (k) => modules[k],
  );
  return `<div class="page-title"><div><p class="eyebrow">ZERON / CONTRIBUTION MANAGER</p><h1>Good changes.<br><em>A clear path to ship.</em></h1><p>A shared view of decisions, reviews and the evidence still needed.</p></div><button class="primary" data-view="prs">Find a PR to move forward ↗</button></div><div class="metrics"><button data-view="prs"><strong>${open.length}</strong><span>Open pull requests</span></button><button data-quick-stage="ready-for-review"><strong>${review.length}</strong><span>Ready for review</span></button><button data-quick-stage="ready-to-merge"><strong>${ready.length}</strong><span>Ready to merge</span></button><div><strong>${urgent.length}</strong><span>Urgent changes</span></div></div><div class="dashboard">${order.map((key) => `<section class="dashboard-panel" draggable="true" data-panel="${key}"><div class="panel-heading"><div><h2>${modules[key].title}</h2><p>${modules[key].hint}</p></div><button class="drag-handle" data-panel-move="${key}" title="Move this section earlier" aria-label="Move ${modules[key].title} earlier">⠿</button></div>${modules[key].items.map((i) => row(i)).join("") || '<div class="empty">Nothing here yet.</div>'}</section>`).join("")}</div><div class="intake"><div><h2>Found a problem? Have an idea?</h2><p>You can submit a report without a GitHub account.</p></div><button data-report="bug">Report a bug</button><button data-report="feature">Propose a feature</button></div>`;
}
function availability(action, item) {
  if (!state.actor) return "Sign in with GitHub to contribute";
  if (!active(item)) return "This item is resolved";
  const needs = {
    triage: "triager",
    direction: "triager",
    review: "reviewer",
    validate: "validator",
    "risk-approval": "maintainer",
    hotfix: "maintainer",
  };
  if (needs[action] && !can(needs[action]))
    return `Requires ${needs[action]} access`;
  if (
    ["review", "validate", "claim", "risk-approval"].includes(action) &&
    (item.originalAuthor || item.author) === state.actor
  )
    return "A different contributor must verify this change";
  if (action === "direction" && item.risk !== "low" && !can("maintainer"))
    return "A maintainer decides direction for this risk level";
  if (action === "validate" && !item.platforms.length)
    return "Triage has no required platforms";
  return "";
}
function actionButton(action, text, item, primary = false) {
  const why = availability(action, item);
  return `<button data-action="${action}" class="${primary ? "primary" : ""}" ${why ? "disabled" : ""} title="${h(why)}">${h(text)}</button>`;
}
function nextAction(item) {
  if (!active(item))
    return {
      title: item.merged ? "This change shipped" : "This item is closed",
      help: "The full history remains available below.",
      action: null,
    };
  if (item.kind === "issue")
    return {
      title: "Triage this report",
      help: "Confirm the problem or proposal, set priority and choose Now, Next or Later.",
      action: "triage",
      button: "Triage report",
    };
  if (!item.triaged)
    return {
      title: "Define the scope and test coverage",
      help: "Choose the area, risk, priority and platforms that need validation.",
      action: "triage",
      button: "Triage PR",
    };
  if (item.direction !== "accepted")
    return {
      title: "Decide whether this change belongs",
      help: "Accept the direction, defer it or record why it should not proceed.",
      action: "direction",
      button: "Decide direction",
    };
  if (item.state === "changes-requested")
    return {
      title: "Address the requested changes",
      help: "The author needs to resolve the review findings. A fresh review can then verify the current revision.",
      action: "review",
      button: "Review the changes",
    };
  if (item.blockers.some((b) => b.includes("code review")))
    return {
      title: "Review the current revision",
      help: "Claim the code review, inspect the diff and record your findings.",
      action: "review",
      button: "Submit code review",
    };
  if (item.blockers.some((b) => b.includes("risk approval")))
    return {
      title: "Approve the high-risk revision",
      help: "A maintainer must inspect this exact revision and explain the approval.",
      action: "risk-approval",
      button: "Approve risk",
    };
  if (item.blockers.some((b) => b.toLowerCase().includes("validation")))
    return {
      title: "Complete behavior validation",
      help: "Test the required platforms and record what happened, including failed tests.",
      action: "validate",
      button: "Record validation",
    };
  if (item.blockers.length)
    return {
      title: "Resolve the remaining blockers",
      help: item.blockers.join(" · "),
      action: null,
    };
  return {
    title: "The review packet is complete",
    help: "Check GitHub’s current branch rules and integration state before merging.",
    action: null,
    merge: true,
  };
}
function evidence(records, type, item) {
  return records?.length
    ? records
        .slice()
        .reverse()
        .map(
          (r) =>
            `<article class="evidence"><div>${badge(r.verdict, tone(r.verdict))}<strong>@${h(r.actor)}</strong>${r.platform ? badge(r.platform) : ""}${badge(r.revision === item.revision ? "current revision" : "older revision", r.revision === item.revision ? "" : "amber")}${link(r.url, "GitHub review")}</div><p>${h(r.summary)}</p>${r.environment ? `<p class="muted">${h(r.environment)}</p>` : ""}${link(r.artifact, "Evidence artifact")}<small>${h(time(r.at))} · ${h(short(r.revision))}</small></article>`,
        )
        .join("")
    : `<p class="empty">No ${type} recorded yet.</p>`;
}
function checksTable(checks, upstream = false) {
  return checks?.length
    ? `<div class="checks">${checks.map((c) => `<div>${badge(c.conclusion || c.state || "pending", tone(c.conclusion || c.state))}<span>${h(c.name || c.context)}</span>${link(c.url || c.target_url, "View run")}${!upstream && c.baseRevision === null ? '<small class="muted">Base not attested</small>' : ""}</div>`).join("")}</div>`
    : '<p class="empty">No checks reported. Missing checks do not count as passing.</p>';
}
function detail() {
  const i = selected,
    n = nextAction(i),
    why = n.action ? availability(n.action, i) : "";
  const steps = [
    ["Scope", i.triaged && i.direction === "accepted"],
    [
      "Code review",
      !i.blockers.some((b) => /code review|requested changes/i.test(b)),
    ],
    [
      "Validation",
      !i.blockers.some((b) => /validation|risk approval/i.test(b)),
    ],
    ["Integration", !i.blockers.some((b) => /check|draft/i.test(b))],
    ["Merge", i.merged],
  ];
  const tasks =
    i.kind === "pr"
      ? ["code-review", ...(i.platforms || []).map((p) => "validate:" + p)]
      : ["reproduce"];
  return `<button class="back" data-back>← Back to ${view === "overview" ? "overview" : "the queue"}</button><div class="detail-title"><div><div class="row-meta">${badge(i.state, tone(i.state))}<span>${i.local ? "Report" : "Pull request"} #${h(i.number)}</span>${i.upstreamUrl ? badge("upstream mirror") : ""}</div><h1>${h(i.title)}</h1><p>By @${h(i.upstream?.author || i.author)} ${i.local && !i.verifiedAuthor ? badge("unverified guest", "amber") : ""} · ${link(i.url, "Open on GitHub")} ${link(i.upstreamUrl, "Original upstream PR")}</p></div><button data-star="${h(i.id)}" class="${isStar(i.id) ? "starred" : ""}">${isStar(i.id) ? "★ Starred" : "☆ Star this change"}</button></div>${i.kind === "pr" ? `<ol class="journey">${steps.map(([name, done], idx) => `<li class="${done ? "done" : ""}"><span>${done ? "✓" : idx + 1}</span>${name}</li>`).join("")}</ol>` : ""}<section class="next-action"><div><p class="eyebrow">NEXT STEP</p><h2>${h(n.title)}</h2><p>${h(n.help)}</p>${why ? `<p class="permission-note">${h(why)}${!state.actor && state.loginEnabled ? ' · <a href="/auth/github">Sign in</a>' : ""}</p>` : ""}</div><div>${n.action ? actionButton(n.action, n.button, i, true) : n.merge ? link(i.url, "Open GitHub to merge") : link(i.url, "Inspect on GitHub")}</div></section><div class="action-bar">${actionButton("triage", "Edit triage", i)}${i.kind === "pr" ? [actionButton("direction", "Direction decision", i), actionButton("review", "Code review", i), actionButton("validate", "Behavior validation", i), actionButton("hotfix", "Hotfix path", i)].join("") : ""}</div><div class="detail-grid"><div><section class="panel"><h2>Scope and context</h2><div class="facts">${badge(i.area)}${badge(i.risk + " risk")}${badge(i.priority + " priority")}${(i.platforms || []).map((p) => badge(p)).join("")}</div><p>${i.directionReason ? h(i.directionReason) : "Scope has not been accepted yet."}</p>${i.kind === "pr" ? `<p class="mono">${h(i.headBranch || "head")} → ${h(i.baseBranch || "base")} · ${h(short(i.revision))}</p><p class="muted">${i.changedFiles ?? "—"} files · +${i.additions ?? "—"} / −${i.deletions ?? "—"} · Mergeability: ${i.mergeable === false ? "conflicts" : i.mergeable === true ? "mergeable" : "GitHub is computing"} ${link(i.diffUrl || (i.url && i.url + "/files"), "Inspect diff")}</p>` : ""}<div class="support"><p class="muted">${Object.values(i.votes || {}).filter((v) => v.dimension === "demand").length} demand signals · ${Object.values(i.votes || {}).filter((v) => v.dimension === "urgency").length} urgency signals</p>${actionButton("vote", "Share a use case", i)}</div><details><summary>${i.local ? "Full report" : "PR description"}</summary><div class="description">${h(i.body || "No description supplied.")}</div></details></section>${i.dependencies?.length ? `<section class="panel"><h2>Dependencies</h2>${i.dependencies.map((d) => `<p>Stacked on ${link(d.url, `fork PR #${d.number}`)} · upstream #${d.upstreamNumber}</p>`).join("")}</section>` : ""}<section class="panel"><h2>Code review</h2>${evidence(i.reviews, "code reviews", i)}</section><section class="panel"><h2>Behavior validation</h2>${evidence(i.validations, "platform tests", i)}</section><section class="panel"><h2>Fork checks</h2>${checksTable(i.checks)}<p class="note">Required: ${h(state.policy.requiredChecks.join(", "))}. Evidence belongs to a specific revision.</p></section>${i.upstream ? `<section class="panel upstream"><h2>Upstream context</h2><p class="muted">Imported ${h(time(i.upstream.fetchedAt))}</p><p>These results belong to the original PR. They do not approve or test this fork mirror.</p>${link(i.upstreamUrl, "View original PR")}<h3>Upstream checks</h3>${checksTable(i.upstream.checks, true)}<h3>Upstream reviews</h3>${evidence(i.upstream.reviews, "upstream reviews", { revision: i.upstream.revision })}</section>` : ""}</div><aside><section class="panel"><h2>Remaining requirements</h2>${i.blockers.length ? `<ul class="blockers">${i.blockers.map((b) => `<li>${h(b)}</li>`).join("")}</ul>` : '<p class="green-text">✓ Review packet complete</p>'}</section><section class="panel"><h2>Claim a task</h2><p class="muted">Let others know what you are working on.</p>${tasks
    .map((task) => {
      const claim = i.claims?.find((c) => c.task === task),
        deny = availability("claim", i);
      return `<div class="task"><strong>${h(label(task))}</strong><p>${claim ? `@${h(claim.actor)} · expires ${h(time(claim.expires))}` : "Available"}</p>${claim && claim.actor === state.actor ? `<button data-task="${h(task)}" data-task-action="renew">Renew</button><button data-task="${h(task)}" data-task-action="release">Release</button>` : claim ? "<button disabled>Claimed</button>" : `<button data-task="${h(task)}" data-task-action="claim" ${deny ? "disabled" : ""} title="${h(deny)}">Claim task</button>`}</div>`;
    })
    .join(
      "",
    )}</section><section class="panel"><h2>Activity</h2>${i.events?.length ? i.events.map((e) => `<article class="activity"><strong>${h(label(e.action))}</strong><span>@${h(e.actor)} · ${h(time(e.at))}</span>${e.data?.reason ? `<p>${h(e.data.reason)}</p>` : ""}</article>`).join("") : '<p class="empty">No workflow actions yet.</p>'}</section></aside></div>`;
}
function render() {
  app.innerHTML = `${nav()}<div class="workspace">${header()}<main>${selected ? detail() : view === "overview" ? overview() : listView()}<footer>${state.preview ? "Sandbox context" : `GitHub synced ${h(time(state.lastReconciled))}`} ${state.syncError ? `· ${h(state.syncError.message)}` : ""}${state.writebackError ? `· ${h(state.writebackError.message)}` : ""}${can("maintainer") && !state.preview && state.mode !== "demo" ? "<button data-sync>Refresh GitHub</button>" : ""}</footer></main></div>`;
  bind();
}
function field(name, title, value = "", type = "text", hint = "") {
  return `<label>${title}${type === "textarea" ? `<textarea name="${name}" required rows="4">${h(value)}</textarea>` : `<input name="${name}" type="${type}" value="${h(value)}" ${name === "artifact" ? "" : "required"}>`}${hint ? `<small>${hint}</small>` : ""}</label>`;
}
function choice(name, title, options, current = "") {
  return `<fieldset><legend>${title}</legend><div class="choices">${options.map(([value, title]) => `<label><input type="radio" name="${name}" value="${h(value)}" ${value === current ? "checked" : ""} required><span>${h(title)}</span></label>`).join("")}</div></fieldset>`;
}
function coverage(item) {
  return `<fieldset><legend>Required platform tests</legend><div class="choices">${["linux", "macos", "windows", "ios", "android"].map((p) => `<label><input type="checkbox" name="platforms" value="${p}" ${item.platforms?.includes(p) ? "checked" : ""}><span>${p}</span></label>`).join("")}</div><small>Choose the affected platforms. Leave empty only when behavior tests are unnecessary.</small></fieldset>`;
}
function showDialog(title, description, content, submit, onSubmit) {
  closeDialog();
  const dialog = document.createElement("dialog");
  dialog.className = "action-dialog";
  dialog.setAttribute("aria-labelledby", "dialog-title");
  dialog.innerHTML = `<form><div class="dialog-title"><h2 id="dialog-title">${h(title)}</h2><button type="button" data-close aria-label="Close dialog">×</button></div><p class="muted">${h(description)}</p>${content}<p class="form-error" role="alert"></p><div class="dialog-footer"><button type="button" data-close>Cancel</button>${submit ? `<button type="submit" class="primary">${h(submit)}</button>` : ""}</div></form>`;
  document.body.append(dialog);
  modal = dialog;
  dialog.showModal();
  dialog
    .querySelectorAll("[data-close]")
    .forEach((b) => (b.onclick = closeDialog));
  dialog.addEventListener("cancel", closeDialog);
  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) {
      const r = dialog.getBoundingClientRect();
      if (
        e.clientX < r.left ||
        e.clientX > r.right ||
        e.clientY < r.top ||
        e.clientY > r.bottom
      )
        closeDialog();
    }
  });
  dialog.querySelector("form").onsubmit = async (event) => {
    event.preventDefault();
    const button = dialog.querySelector("[type=submit]");
    if (busy) return;
    busy = true;
    if (button) button.disabled = true;
    try {
      await onSubmit(new FormData(event.target));
      closeDialog();
    } catch (e) {
      dialog.querySelector(".form-error").textContent = e.message;
    } finally {
      busy = false;
      if (button) button.disabled = false;
    }
  };
}
function closeDialog() {
  if (modal) {
    modal.close();
    modal.remove();
    modal = null;
  }
}
async function mutate(action, body) {
  selected = await api(`/api/items/${selected.id}/${action}`, {
    revision: selected.revision,
    ...body,
  });
  await refresh();
  toast("Saved. The next step is updated.");
}
function actionDialog(action) {
  const i = selected,
    deny = availability(action, i);
  if (deny) {
    toast(deny);
    return;
  }
  const titles = {
    triage: "Triage this change",
    direction: "Decide direction",
    review: "Submit code review",
    validate: "Record behavior validation",
    "risk-approval": "Approve the high-risk revision",
    hotfix: "Prepare an urgent hotfix",
    vote: "Share a use case",
  };
  let content = "",
    submit = "Save";
  if (action === "triage" || action === "hotfix") {
    content =
      field("area", "Area", i.area === "untriaged" ? "" : i.area) +
      (action === "triage"
        ? choice(
            "risk",
            "Risk",
            [
              ["low", "Low"],
              ["medium", "Medium"],
              ["high", "High"],
            ],
            i.risk,
          ) +
          choice(
            "priority",
            "Priority",
            [
              ["urgent", "Urgent"],
              ["normal", "Normal"],
              ["low", "Low"],
            ],
            i.priority,
          )
        : '<p class="callout">This marks the PR as low risk and urgent, and accepts its scope. Independent review, tests and GitHub merge rules still apply.</p>') +
      coverage(i) +
      (i.kind === "issue"
        ? choice(
            "plan",
            "Roadmap placement",
            [
              ["needs-triage", "Needs triage"],
              ["now", "Now"],
              ["next", "Next"],
              ["later", "Later"],
            ],
            i.plan,
          )
        : "") +
      (action === "hotfix"
        ? field("reason", "Hotfix scope and reason", "", "textarea")
        : field("blocker", "Blocker (optional)", i.blocker || ""));
    submit = action === "hotfix" ? "Accept hotfix scope" : "Save triage";
  }
  if (action === "direction") {
    content =
      choice(
        "verdict",
        "Decision",
        [
          ["accepted", "Accept direction"],
          ["needs-decision", "Needs a maintainer"],
          ["deferred", "Defer"],
          ["rejected", "Decline"],
        ],
        i.direction === "pending" ? "" : i.direction,
      ) + field("reason", "Scope and decision reason", "", "textarea");
    submit = "Record decision";
  }
  if (action === "review") {
    content =
      `<p class="revision-note">Reviewing revision <code>${h(short(i.revision))}</code> · ${link(i.diffUrl || i.url + "/files", "Inspect diff")}</p>` +
      choice(
        "verdict",
        "Review result",
        [
          ["pass", "Approve"],
          ["changes", "Request changes"],
        ],
        "",
      ) +
      field("summary", "Findings and scope reviewed", "", "textarea");
    submit = "Submit review";
  }
  if (action === "validate") {
    content =
      choice(
        "platform",
        "Platform",
        i.platforms.map((p) => [p, p]),
        i.platforms.length === 1 ? i.platforms[0] : "",
      ) +
      choice(
        "verdict",
        "Result",
        [
          ["pass", "Passed"],
          ["fail", "Failed"],
        ],
        "",
      ) +
      field("environment", "Environment and build") +
      field(
        "summary",
        "Reproduction, steps exercised, and results",
        "",
        "textarea",
      ) +
      field("artifact", "Evidence link (optional)", "", "url");
    submit = "Record validation";
  }
  if (action === "vote") {
    content =
      choice("dimension", "Community signal", [
        ["demand", "I need this"],
        ["urgency", "This is urgent"],
      ]) + field("reason", "Your use case or reason", "", "textarea");
    submit = "Share signal";
  }
  if (action === "risk-approval") {
    content = field(
      "reason",
      "Approval reason for this revision",
      "",
      "textarea",
    );
    submit = "Approve risk";
  }
  showDialog(
    titles[action],
    `This record applies to revision ${short(i.revision)}.`,
    content,
    submit,
    async (data) => {
      const body = Object.fromEntries(data);
      if (["triage", "hotfix"].includes(action))
        body.platforms = data.getAll("platforms");
      await mutate(action, body);
    },
  );
  modal.querySelector('[name="blocker"]')?.removeAttribute("required");
}
function reportDialog(type) {
  showDialog(
    type === "bug" ? "Report a bug" : "Propose a feature",
    state.actor
      ? `Submitted as @${state.actor}.`
      : "You don't need an account. Your preferred name is saved in this browser; guest reports are marked unverified.",
    `${!state.actor ? field("name", "Your display name", localStorage.getItem("cm-name") || "") : ""}${field("title", type === "bug" ? "What went wrong?" : "What would you like to add?")}${field("details", type === "bug" ? "Steps to reproduce, expected and actual behavior" : "Who benefits, and what should it do?", "", "textarea")}`,
    "Submit report",
    async (data) => {
      const values = Object.fromEntries(data);
      if (values.name) localStorage.setItem("cm-name", values.name);
      const report = await api("/api/reports", { type, ...values });
      selected = await api(`/api/items/${report.id}`);
      await refresh();
      history.replaceState({}, "", `/?item=${encodeURIComponent(report.id)}`);
      toast("Report submitted. It is ready for triage.");
    },
  );
}
function previewDialog() {
  const demo = state.mode === "demo";
  showDialog(
    "Try an authorization level",
    demo
      ? "Use the local test fixtures."
      : "This fork-only sandbox uses real PR context. Changes stay in a separate database and never write to GitHub.",
    choice(
      "role",
      "View as",
      [
        ["guest", "Guest"],
        ["contributor", "Contributor"],
        ["triager", "Triager"],
        ["reviewer", "Reviewer"],
        ["validator", "Validator"],
        ["maintainer", "Maintainer"],
      ],
      state.actor?.replace("preview-", "").replace("demo-", "") || "guest",
    ),
    "Open role preview",
    async (data) => {
      if (demo) {
        const role = data.get("role");
        if (role === "guest") {
          if (state.actor) await api("/api/logout", {});
        } else
          await api("/api/demo/login", {
            actor: "demo-" + (role === "contributor" ? "author" : role),
          });
      } else await api("/api/dev/preview", { role: data.get("role") });
      await refresh();
    },
  );
}
async function open(id) {
  selected = await api(`/api/items/${id}`);
  history.pushState({}, "", `/?item=${encodeURIComponent(id)}`);
  render();
  window.scrollTo(0, 0);
}
async function star(id) {
  if (state.actor) {
    await api("/api/stars", { id, starred: !isStar(id) });
    await refresh();
  } else {
    let values = stars();
    values = isStar(id) ? values.filter((v) => v !== id) : [...values, id];
    localStorage.setItem("cm-stars", JSON.stringify(values));
    render();
  }
  toast(
    isStar(id)
      ? "Added to your starred changes."
      : "Removed from your starred changes.",
  );
}
function bind() {
  app
    .querySelectorAll("[data-open]")
    .forEach(
      (b) =>
        (b.onclick = () => open(b.dataset.open).catch((e) => toast(e.message))),
    );
  app.querySelectorAll("[data-view]").forEach(
    (b) =>
      (b.onclick = () => {
        selected = null;
        view = b.dataset.view;
        stage = "all";
        query = "";
        history.pushState({}, "", "/");
        render();
      }),
  );
  app
    .querySelectorAll("[data-star]")
    .forEach(
      (b) =>
        (b.onclick = () => star(b.dataset.star).catch((e) => toast(e.message))),
    );
  app
    .querySelectorAll("[data-report]")
    .forEach((b) => (b.onclick = () => reportDialog(b.dataset.report)));
  app
    .querySelectorAll("[data-action]")
    .forEach((b) => (b.onclick = () => actionDialog(b.dataset.action)));
  app.querySelectorAll("[data-stage]").forEach(
    (b) =>
      (b.onclick = () => {
        stage = b.dataset.stage;
        render();
      }),
  );
  app.querySelectorAll("[data-layout]").forEach(
    (b) =>
      (b.onclick = () => {
        layout = b.dataset.layout;
        localStorage.setItem("cm-layout", layout);
        render();
      }),
  );
  app.querySelectorAll("[data-quick-stage]").forEach(
    (b) =>
      (b.onclick = () => {
        view = "prs";
        stage = b.dataset.quickStage;
        render();
      }),
  );
  app
    .querySelectorAll("[data-task]")
    .forEach(
      (b) =>
        (b.onclick = () =>
          mutate(b.dataset.taskAction, { task: b.dataset.task }).catch((e) =>
            toast(e.message),
          )),
    );
  app.querySelector("[data-preview]")?.addEventListener("click", previewDialog);
  app.querySelector("[data-live]")?.addEventListener("click", async () => {
    await api("/api/dev/preview", { role: "live" });
    await refresh();
  });
  app.querySelector("[data-logout]")?.addEventListener("click", async () => {
    await api("/api/logout", {});
    await refresh();
  });
  app.querySelector("[data-back]")?.addEventListener("click", () => {
    selected = null;
    history.pushState({}, "", "/");
    render();
  });
  app.querySelector("[data-sync]")?.addEventListener("click", async () => {
    await api("/api/sync", {});
    toast("GitHub refresh started.");
  });
  app.querySelector("#search")?.addEventListener("input", (e) => {
    query = e.target.value;
    const pos = e.target.selectionStart;
    render();
    const input = app.querySelector("#search");
    input.focus();
    input.setSelectionRange(pos, pos);
  });
  app.querySelectorAll("[data-drag-id][draggable]").forEach(
    (card) =>
      (card.ondragstart = (e) => {
        dragItem = card.dataset.dragId;
        e.dataTransfer.setData("text/plain", dragItem);
      }),
  );
  app.querySelectorAll("[data-drop-stage]").forEach((column) => {
    column.ondragover = (e) => {
      e.preventDefault();
      column.classList.add("drag-over");
    };
    column.ondragleave = () => column.classList.remove("drag-over");
    column.ondrop = async (e) => {
      e.preventDefault();
      column.classList.remove("drag-over");
      const id = e.dataTransfer.getData("text/plain") || dragItem;
      if (!state.items.some((i) => i.id === id)) return;
      await open(id);
      const actions = {
        "needs-triage": "triage",
        "needs-direction": "direction",
        "ready-for-review": "direction",
        "changes-requested": "review",
        "needs-validation": "review",
        "ready-to-merge": "validate",
      };
      const target = column.dataset.dropStage;
      if (target === "ready-to-merge" && selected.state === "ready-to-merge") {
        toast("The packet is ready. Open GitHub to merge.");
        return;
      }
      actionDialog(actions[target]);
    };
  });
  app.querySelectorAll("[data-panel]").forEach((panel) => {
    panel.ondragstart = (e) => {
      if (e.target.closest("[data-drag-id]")) return;
      e.dataTransfer.setData("cm-panel", panel.dataset.panel);
    };
    panel.ondragover = (e) => {
      if (!e.dataTransfer.types.includes("cm-panel")) return;
      e.preventDefault();
    };
    panel.ondrop = (e) => {
      const key = e.dataTransfer.getData("cm-panel");
      if (!key) return;
      e.preventDefault();
      const order = [...app.querySelectorAll("[data-panel]")]
        .map((p) => p.dataset.panel)
        .filter((k) => k !== key);
      order.splice(order.indexOf(panel.dataset.panel), 0, key);
      localStorage.setItem("cm-panels", JSON.stringify(order));
      render();
    };
  });
  app.querySelectorAll("[data-panel-move]").forEach(
    (button) =>
      (button.onclick = () => {
        const order = [...app.querySelectorAll("[data-panel]")].map(
            (p) => p.dataset.panel,
          ),
          idx = order.indexOf(button.dataset.panelMove);
        if (idx > 0)
          [order[idx], order[idx - 1]] = [order[idx - 1], order[idx]];
        localStorage.setItem("cm-panels", JSON.stringify(order));
        render();
      }),
  );
}
window.addEventListener("popstate", async () => {
  const id = new URL(location.href).searchParams.get("item");
  selected = id ? await api(`/api/items/${id}`) : null;
  render();
});
try {
  state = await api("/api/state");
  const id = new URL(location.href).searchParams.get("item");
  if (id && /^(pr|issue|report):\d+$/.test(id))
    selected = await api(`/api/items/${id}`);
  render();
} catch (e) {
  app.innerHTML = `<main class="loading"><h1>Couldn't load the workspace</h1><p>${h(e.message)}</p><button data-reload>Reload</button></main>`;
}
document
  .querySelector("[data-reload]")
  ?.addEventListener("click", () => location.reload());
setInterval(() => {
  if (
    !modal &&
    !["INPUT", "TEXTAREA"].includes(document.activeElement?.tagName)
  )
    refresh().catch(() => {});
}, 30000);
