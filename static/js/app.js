// turaes M0 dashboard shell.
//
// Zero-build vanilla JS for the scaffold; the zero-build Preact/HTM UI lands
// with the monitoring milestone (see docs/ROADMAP.md). Reads the authenticated
// JSON API with same-origin cookies.

const $ = (sel) => document.querySelector(sel);

async function api(path, options = {}) {
  const res = await fetch(path, {
    headers: { "content-type": "application/json" },
    credentials: "same-origin",
    ...options,
  });
  if (res.status === 204) return null;
  const body = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(body.detail || `HTTP ${res.status}`);
  return body;
}

function badge(status) {
  return `<span class="badge ${status}">${status}</span>`;
}

function stat(label, value) {
  return `<div class="stat"><span>${label}</span><span>${value}</span></div>`;
}

function fmtBytes(n) {
  if (!n) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(1)} ${units[i]}`;
}

async function loadUser() {
  try {
    const user = await api("/auth/me");
    $("#user").innerHTML = `${user.login}${user.name ? ` <span class="muted">(${user.name})</span>` : ""}`;
  } catch {
    $("#user").innerHTML = `<a class="btn small" href="/auth/login">Sign in</a>`;
  }
}

async function loadStats(app) {
  const [metrics, visitors] = await Promise.all([
    api(`/api/v1/apps/${app.id}/stats?hours=1`).catch(() => ({ metrics: [] })),
    api(`/api/v1/apps/${app.id}/visitors?hours=24`).catch(() => ({ visitors: [] })),
  ]);
  const last = metrics.metrics?.at(-1);
  const visits = (visitors.visitors || []).reduce((acc, v) => acc + (v.visits || 0), 0);
  const uniques = (visitors.visitors || []).reduce((acc, v) => Math.max(acc, v.uniques || 0), 0);
  return { cpu: last ? `${last.cpu_pct.toFixed(1)}%` : "—", mem: last ? fmtBytes(last.mem_bytes) : "—", visits, uniques };
}

async function renderApps() {
  const container = $("#apps");
  try {
    const { applications } = await api("/api/v1/apps");
    if (!applications.length) {
      container.innerHTML = `<p class="muted">No applications yet. Add one below.</p>`;
      return;
    }
    const cards = await Promise.all(applications.map(async (app) => {
      const s = await loadStats(app);
      return `
        <div class="card">
          <h3>${app.name} ${badge(app.status)}</h3>
          ${stat("domain", app.domain || "—")}
          ${stat("port", app.port)}
          ${stat("runtime", app.runtime)}
          ${stat("cpu", s.cpu)}
          ${stat("memory", s.mem)}
          ${stat("visits 24h", s.visits)}
          ${stat("unique 24h", s.uniques)}
          <button class="btn small" data-deploy="${app.id}">Deploy</button>
        </div>`;
    }));
    container.innerHTML = cards.join("");
    container.querySelectorAll("[data-deploy]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        btn.disabled = true;
        btn.textContent = "Deploying…";
        try {
          await api(`/api/v1/apps/${btn.dataset.deploy}/deploy`, { method: "POST" });
        } catch (e) {
          alert(e.message);
        }
        renderApps();
      });
    });
  } catch (e) {
    container.innerHTML = `<p class="muted">${e.message}</p>`;
  }
}

$("#refresh").addEventListener("click", renderApps);

$("#create-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const fd = new FormData(ev.target);
  const payload = Object.fromEntries([...fd.entries()].filter(([, v]) => v !== ""));
  payload.port = Number(payload.port);
  try {
    await api("/api/v1/apps", { method: "POST", body: JSON.stringify(payload) });
    $("#create-msg").textContent = `Created ${payload.name}`;
    ev.target.reset();
    renderApps();
  } catch (e) {
    $("#create-msg").textContent = e.message;
  }
});

loadUser();
renderApps();
