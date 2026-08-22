// Dashboard + museum: read the REST endpoints and render champion, Elo
// sparkline, training stats, museum list, and the away report. Read-only.

export interface ChampionPayload {
  id: number;
  genome_id: number;
  generation: number;
  crowned_at: number;
  dethroned_at: number | null;
  reigning: boolean;
  gauntlet_record: unknown;
  era: string | null;
  elo: number | null;
}

interface EloPoint {
  genome_id: number;
  elo: number;
  at: number;
}

interface TrainingStat {
  generation: number;
  matches_run: number;
  pop_fitness_mean: number;
  pop_fitness_best: number;
  diversity: number;
  at: number;
}

interface RecentEvent {
  kind: string;
  payload: unknown;
  at: number;
}

interface StatusPayload {
  ok: boolean;
  uptime_secs: number;
  counts: { matches: number; genomes: number; champions: number; events: number };
  recent_events: RecentEvent[];
  trainer: { running: boolean; generation: number | null; matches_run: number };
}

async function get<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: ${res.status}`);
  return res.json() as Promise<T>;
}

export function initDashboard(): void {
  document.getElementById("open-dashboard")?.addEventListener("click", () => {
    void show();
  });
  document.getElementById("dash-back")?.addEventListener("click", hide);
}

export function hide(): void {
  document.getElementById("dashboard")?.classList.add("hidden");
  document.getElementById("lobby")?.classList.remove("hidden");
  document.getElementById("overlay")?.classList.remove("hidden");
}

async function show(): Promise<void> {
  document.getElementById("lobby")?.classList.add("hidden");
  document.getElementById("result")?.classList.add("hidden");
  const dash = document.getElementById("dashboard");
  dash?.classList.remove("hidden");
  document.getElementById("overlay")?.classList.remove("hidden");

  setText("dash-status", "loading…");
  try {
    const [status, champion, stats, museum, eloHist] = await Promise.all([
      get<StatusPayload>("/api/status"),
      get<{ champion: ChampionPayload | null }>("/api/champion"),
      get<{ stats: TrainingStat[] }>("/api/training-stats"),
      get<{ champions: ChampionPayload[] }>("/api/museum"),
      get<{ points: EloPoint[] }>("/api/elo-history"),
    ]);

    renderChampion(champion.champion);
    renderElo(eloHist.points);
    renderStats(status.trainer, stats.stats);
    renderMuseum(museum.champions);
    renderEvents(status.recent_events ?? []);
    setText("dash-status", "");
  } catch (e) {
    setText("dash-status", `error: ${(e as Error).message}`);
  }
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

function renderChampion(c: ChampionPayload | null): void {
  const elt = document.getElementById("dash-champion");
  if (!elt) return;
  elt.replaceChildren();
  if (!c) {
    elt.appendChild(el("p", "muted", "No champion yet — the trainer has not crowned one."));
    return;
  }
  const champ = el("div", "champ");
  champ.appendChild(el("strong", undefined, `Champion #${c.genome_id}`));
  champ.appendChild(el("span", undefined, `generation ${c.generation}`));
  champ.appendChild(el("span", undefined, `Elo ${c.elo == null ? "—" : String(Math.round(c.elo))}`));
  if (c.era) champ.appendChild(el("span", undefined, c.era));
  champ.appendChild(
    el("span", "muted", `crowned ${new Date(c.crowned_at * 1000).toLocaleString()}`),
  );
  elt.appendChild(champ);
}

function renderElo(points: EloPoint[]): void {
  const elt = document.getElementById("dash-elo");
  if (!elt) return;
  elt.replaceChildren();
  if (points.length < 2) {
    elt.textContent = "Not enough league matches for an Elo graph yet.";
    return;
  }
  const vals = points.map((p) => p.elo);
  const min = Math.min(...vals);
  const max = Math.max(...vals);
  const span = max - min || 1;
  const w = 320;
  const h = 60;
  elt.appendChild(
    el(
      "div",
      "muted",
      `Champion Elo over time (${Math.round(vals[0])} → ${Math.round(vals[vals.length - 1])})`,
    ),
  );
  const canvas = el("canvas", "spark");
  canvas.width = w;
  canvas.height = h;
  canvas.setAttribute("aria-label", "Champion Elo over time");
  elt.appendChild(canvas);

  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  ctx.imageSmoothingEnabled = false;
  ctx.fillStyle = "#070a0e";
  ctx.fillRect(0, 0, w, h);
  ctx.strokeStyle = "#c8920e";
  ctx.lineWidth = 2;
  ctx.beginPath();
  vals.forEach((v, i) => {
    const x = Math.floor((i / (vals.length - 1)) * (w - 2)) + 1;
    const y = Math.floor(h - ((v - min) / span) * (h - 8) - 4);
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  });
  ctx.stroke();
}

function renderStats(trainer: StatusPayload["trainer"], stats: TrainingStat[]): void {
  const elt = document.getElementById("dash-stats");
  if (!elt) return;
  const latest = stats[stats.length - 1];
  elt.replaceChildren(
    el(
      "div",
      "muted",
      `Trainer: ${trainer.running ? "running" : "idle"} · generation ${trainer.generation ?? 0} · ${(
        trainer.matches_run ?? 0
      ).toLocaleString()} matches`,
    ),
  );
  if (latest) {
    elt.appendChild(
      el(
        "div",
        undefined,
        `latest gen ${latest.generation}: fitness mean ${latest.pop_fitness_mean.toFixed(3)} / best ${latest.pop_fitness_best.toFixed(3)} · diversity ${latest.diversity.toFixed(3)}`,
      ),
    );
  }
}

function renderMuseum(champions: ChampionPayload[]): void {
  const elt = document.getElementById("dash-museum");
  if (!elt) return;
  elt.replaceChildren();
  elt.appendChild(el("h2", undefined, "Museum"));
  if (champions.length === 0) {
    elt.appendChild(el("div", "muted", "Museum is empty."));
    return;
  }
  const table = el("table");
  const thead = document.createElement("thead");
  const headRow = document.createElement("tr");
  for (const cell of ["genome", "gen", "elo", "era", ""]) {
    const th = document.createElement("th");
    th.textContent = cell;
    headRow.appendChild(th);
  }
  thead.appendChild(headRow);
  table.appendChild(thead);

  const tbody = document.createElement("tbody");
  // Last 12 champions, most-recent first (no mutating `.reverse()`).
  const start = Math.max(0, champions.length - 12);
  for (let i = champions.length - 1; i >= start; i--) {
    const c = champions[i];
    const tr = document.createElement("tr");
    const badge = c.reigning ? "👑 reigning" : "dethroned";
    for (const cell of [
      `#${c.genome_id}`,
      `gen ${c.generation}`,
      `Elo ${c.elo == null ? "—" : String(Math.round(c.elo))}`,
      c.era ?? "",
      badge,
    ]) {
      const td = document.createElement("td");
      td.textContent = cell;
      tr.appendChild(td);
    }
    tbody.appendChild(tr);
  }
  table.appendChild(tbody);
  elt.appendChild(table);
}

function renderEvents(events: RecentEvent[]): void {
  const elt = document.getElementById("dash-events");
  if (!elt) return;
  elt.replaceChildren();
  elt.appendChild(el("h2", undefined, "While you were away"));
  if (events.length === 0) {
    elt.appendChild(el("div", "muted", "Nothing yet."));
    return;
  }
  const table = el("table");
  const tbody = document.createElement("tbody");
  for (const e of events.slice(0, 10)) {
    const tr = document.createElement("tr");
    const when = document.createElement("td");
    when.textContent = new Date(e.at * 1000).toLocaleTimeString();
    const kind = document.createElement("td");
    kind.textContent = e.kind;
    tr.append(when, kind);
    tbody.appendChild(tr);
  }
  table.appendChild(tbody);
  elt.appendChild(table);
}

function setText(id: string, text: string): void {
  const e = document.getElementById(id);
  if (e) e.textContent = text;
}