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
      get<any>("/api/status"),
      get<{ champion: ChampionPayload | null }>("/api/champion"),
      get<{ stats: TrainingStat[] }>("/api/training-stats"),
      get<{ champions: ChampionPayload[] }>("/api/museum"),
      get<{ points: EloPoint[] }>("/api/elo-history"),
    ]);

    renderChampion(champion.champion);
    renderElo(eloHist.points);
    renderStats(status, stats.stats);
    renderMuseum(museum.champions);
    renderEvents(status.recent_events ?? []);
    setText("dash-status", "");
  } catch (e) {
    setText("dash-status", `error: ${(e as Error).message}`);
  }
}

function renderChampion(c: ChampionPayload | null): void {
  const elt = document.getElementById("dash-champion");
  if (!elt) return;
  if (!c) {
    elt.innerHTML = "<p>No champion yet — the trainer has not crowned one.</p>";
    return;
  }
  const elo = c.elo == null ? "—" : String(Math.round(c.elo));
  elt.innerHTML = `
    <div class="champ">
      <strong>Champion #${c.genome_id}</strong>
      <span>generation ${c.generation}</span>
      <span>Elo ${elo}</span>
      ${c.era ? `<span>${c.era}</span>` : ""}
      <span class="muted">crowned ${new Date(c.crowned_at * 1000).toLocaleString()}</span>
    </div>`;
}

function renderElo(points: EloPoint[]): void {
  const elt = document.getElementById("dash-elo");
  if (!elt) return;
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
  elt.innerHTML = `
    <div class="muted">Champion Elo over time (${Math.round(vals[0])} → ${Math.round(vals[vals.length - 1])})</div>
    <canvas width="${w}" height="${h}" class="spark" aria-label="Champion Elo over time"></canvas>`;

  const chart = elt.querySelector("canvas");
  const ctx = chart?.getContext("2d");
  if (!chart || !ctx) return;
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

function renderStats(status: any, stats: TrainingStat[]): void {
  const elt = document.getElementById("dash-stats");
  if (!elt) return;
  const t = status.trainer ?? {};
  const latest = stats[stats.length - 1];
  elt.innerHTML = `
    <div class="muted">Trainer: ${t.running ? "running" : "idle"} · generation ${t.generation ?? 0} · ${(t.matches_run ?? 0).toLocaleString()} matches</div>
    ${latest
      ? `<div>latest gen ${latest.generation}: fitness mean ${latest.pop_fitness_mean.toFixed(3)} / best ${latest.pop_fitness_best.toFixed(3)} · diversity ${latest.diversity.toFixed(3)}</div>`
      : ""}`;
}

function renderMuseum(champions: ChampionPayload[]): void {
  const elt = document.getElementById("dash-museum");
  if (!elt) return;
  if (champions.length === 0) {
    elt.innerHTML = "<div class=\"muted\">Museum is empty.</div>";
    return;
  }
  const rows = [...champions].reverse().slice(0, 12).map((c) => {
    const elo = c.elo == null ? "—" : String(Math.round(c.elo));
    const badge = c.reigning ? "👑 reigning" : "dethroned";
    const era = c.era ? `<td>${c.era}</td>` : "<td></td>";
    return `<tr><td>#${c.genome_id}</td><td>gen ${c.generation}</td><td>Elo ${elo}</td>${era}<td>${badge}</td></tr>`;
  });
  elt.innerHTML = `
    <h2>Museum</h2>
    <table><thead><tr><th>genome</th><th>gen</th><th>elo</th><th>era</th><th></th></tr></thead>
    <tbody>${rows.join("")}</tbody></table>`;
}

function renderEvents(events: Array<{ kind: string; payload: any; at: number }>): void {
  const elt = document.getElementById("dash-events");
  if (!elt) return;
  if (events.length === 0) {
    elt.innerHTML = "<h2>While you were away</h2><div class=\"muted\">Nothing yet.</div>";
    return;
  }
  const rows = events.slice(0, 10).map((e) => {
    const when = new Date(e.at * 1000).toLocaleTimeString();
    return `<tr><td>${when}</td><td>${e.kind}</td></tr>`;
  });
  elt.innerHTML = `<h2>While you were away</h2><table><tbody>${rows.join("")}</tbody></table>`;
}

function setText(id: string, text: string): void {
  const e = document.getElementById(id);
  if (e) e.textContent = text;
}
