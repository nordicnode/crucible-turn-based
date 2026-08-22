// Spectate screen: list stored replays, load one, and step through it
// client-side via the wasm replay shim (full state, no fog). The wasm runs the
// exact same sim as the server, so a replay is reproduced byte-for-byte.
// Playback is frame-per-activation: one activation advances per second at
// 1× speed, while the HUD also shows the player-facing round.

import { Renderer } from "./renderer";
import { World } from "./world";
import { applyFrame, applyMeta, type ReplayMeta } from "./snapshot";
import { frame as wasmFrame, meta as wasmMeta } from "./wasm/loader";

/** Base playback speed: turns per second. */
const TURNS_PER_SEC = 1;
const SPEEDS = [1, 2, 4, 8];

interface ReplaySummary {
  id: number;
  map_seed: number;
  p1_type: string;
  p2_type: string;
  result: string;
  duration_turns: number;
  duration_rounds?: number;
  created_at: number;
}

function el<T extends HTMLElement>(id: string): T {
  return document.getElementById(id) as T;
}

function fmtTurns(turn: number): string {
  return String(Math.max(0, turn));
}

function show(id: string): void {
  el(id).classList.remove("hidden");
}
function hide(id: string): void {
  el(id).classList.add("hidden");
}

class Spectate {
  active = false;
  readonly renderer = new Renderer();
  readonly world = new World();
  private replayJson: string | null = null;
  private meta: ReplayMeta | null = null;
  private turn = 0;
  private renderedTurn = -1;
  private duration = 0;
  private playing = false;
  private speedIndex = 0;
  private lastTime = 0;
  private loading = false;
  private pendingTurn: number | null = null;
  private ore0 = 0;
  private ore1 = 0;
  private round = 1;

  get currentTurn(): number {
    return this.turn;
  }
  get score0(): number {
    return this.ore0;
  }
  get score1(): number {
    return this.ore1;
  }

  init(): void {
    document.getElementById("open-spectate")?.addEventListener("click", () => void this.open());
    el("spectate-list-back").addEventListener("click", () => this.close());
    el("sp-play").addEventListener("click", () => this.toggle());
    el("sp-speed").addEventListener("click", () => this.cycleSpeed());
    el("sp-close").addEventListener("click", () => this.close());
    el("sp-scrub").addEventListener("input", () => {
      this.turn = Number(el<HTMLInputElement>("sp-scrub").value);
      this.updateClock();
      void this.requestFrame(this.turn);
    });
  }

  // --- screen transitions -------------------------------------------------

  async open(): Promise<void> {
    hide("lobby");
    hide("result");
    hide("dashboard");
    hide("sidebar");
    hide("log");
    hide("spectate-bar");
    hide("turn-ribbon");
    show("overlay");
    show("spectate-list");
    this.active = false;
    this.setStatus("loading replays…");
    this.renderList([]);
    try {
      const res = await fetch("/api/replays");
      if (!res.ok) throw new Error(`replays: ${res.status}`);
      const data = (await res.json()) as { matches: ReplaySummary[] };
      this.renderList(data.matches ?? []);
    } catch (e) {
      this.renderList([], `error: ${String(e)}`);
    }
  }

  async loadReplay(id: number): Promise<void> {
    this.setStatus("loading replay #" + id + "…");
    try {
      const res = await fetch(`/api/replay/${id}`);
      if (!res.ok) throw new Error(`replay: ${res.status}`);
      const data = (await res.json()) as { replay: string };
      this.replayJson = data.replay;
      this.meta = await wasmMeta(data.replay);
      applyMeta(this.world, this.meta);
      this.duration = Math.max(1, this.meta.duration_turns);

      const scrub = el<HTMLInputElement>("sp-scrub");
      scrub.max = String(this.duration);
      scrub.value = "0";

      // Camera on player 0's HQ, like a live match start.
      const hq = this.meta.hq_tiles[0];
      this.renderer.camera.centerOn(
        hq[0] + 0.5,
        hq[1] + 0.5,
        window.innerWidth,
        window.innerHeight,
        18,
      );

      hide("overlay");
      hide("spectate-list");
      show("spectate-bar");
      this.active = true;
      this.playing = false;
      this.turn = 0;
      this.renderedTurn = -1;
      this.lastTime = performance.now();
      await this.requestFrame(0);
    } catch (e) {
      this.setStatus(`error: ${String(e)}`);
    }
  }

  close(): void {
    this.active = false;
    this.playing = false;
    this.replayJson = null;
    this.meta = null;
    hide("spectate-bar");
    hide("spectate-list");
    hide("dashboard");
    show("overlay");
    show("lobby");
  }

  // --- controls -----------------------------------------------------------

  toggle(): void {
    if (this.replayJson == null) return;
    this.playing = !this.playing;
    this.lastTime = performance.now();
    el("sp-play").textContent = this.playing ? "PAUSE" : "PLAY";
    if (this.playing && this.turn >= this.duration) this.turn = 0;
  }

  cycleSpeed(): void {
    this.speedIndex = (this.speedIndex + 1) % SPEEDS.length;
    el("sp-speed").textContent = `${SPEEDS[this.speedIndex]}×`;
  }

  // --- per-frame ----------------------------------------------------------

  draw(ctx: CanvasRenderingContext2D, w: number, h: number): void {
    const now = performance.now();
    const dt = Math.min(0.25, (now - this.lastTime) / 1000);
    this.lastTime = now;

    if (this.playing && this.replayJson != null && !this.loading) {
      // Freeze the clock while a wasm frame is in flight: otherwise `turn`
      // keeps climbing during a slow frame load and the HUD steward overshoots
      // the rendered turn, causing a rubber-band / out-of-order scrub.
      this.turn += dt * TURNS_PER_SEC * SPEEDS[this.speedIndex];
      if (this.turn >= this.duration) {
        this.turn = this.duration;
        this.playing = false;
        el("sp-play").textContent = "PLAY";
      }
      const t = Math.floor(this.turn);
      if (t !== this.renderedTurn) {
        this.updateClock();
        void this.requestFrame(t);
      }
    }

    this.renderer.draw(ctx, this.world, new Set(), w, h);
    const scrub = el<HTMLInputElement>("sp-scrub");
    if (!this.loading) scrub.value = String(Math.floor(this.turn));
  }

  private async requestFrame(t: number): Promise<void> {
    if (this.loading) {
      this.pendingTurn = t;
      return;
    }
    if (this.replayJson == null) return;
    this.loading = true;
    try {
      const f = await wasmFrame(this.replayJson, t);
      applyFrame(this.world, f);
      this.renderedTurn = f.turn;
      this.ore0 = f.ore0;
      this.ore1 = f.ore1;
      this.round = f.round ?? Math.max(1, Math.floor((f.turn + 1) / 2));
      this.updateHud();
    } catch (e) {
      // A corrupt/legacy replay must degrade to a visible error, never crash
      // the page (the wasm shim returns errors instead of panicking).
      this.playing = false;
      this.active = false;
      this.replayJson = null;
      hide("spectate-bar");
      show("overlay");
      show("spectate-list");
      this.setStatus(`replay error at turn ${t}: ${String(e)}`);
    } finally {
      this.loading = false;
    }
    if (this.pendingTurn != null) {
      const p = this.pendingTurn;
      this.pendingTurn = null;
      void this.requestFrame(p);
    }
  }

  private updateClock(): void {
    const currentRound = this.renderedTurn >= 0
      ? this.round
      : Math.max(1, Math.floor((this.turn + 1) / 2));
    const durationRounds = this.meta?.duration_rounds
      ?? Math.max(1, Math.floor((this.duration + 1) / 2));
    el("sp-clock").textContent = `R${fmtTurns(currentRound)} / R${fmtTurns(durationRounds)} · T${fmtTurns(this.turn)} / T${fmtTurns(this.duration)}`;
  }

  private updateHud(): void {
    this.updateClock();
    const won = this.world.result?.winner;
    const result =
      won == null ? "" : won === 0 ? " — P0 wins" : " — P1 wins";
    el("sp-score").textContent = `P0 ${this.ore0} · P1 ${this.ore1}${result}`;
  }

  // --- list rendering -----------------------------------------------------

  private setStatus(s: string): void {
    el("spectate-status").textContent = s;
  }

  private renderList(matches: ReplaySummary[], error?: string): void {
    this.setStatus(error ?? (matches.length === 0 ? "No replays yet — play a match first." : ""));
    const body = el("spectate-list-body");
    body.innerHTML = "";
    for (const m of matches) {
      const row = document.createElement("button");
      row.className = "btn spectate-row";
      const when = new Date(m.created_at * 1000).toLocaleString();
      const rounds = m.duration_rounds ?? Math.max(1, Math.floor((m.duration_turns + 1) / 2));
      row.textContent =
        `#${m.id} ${m.p1_type} vs ${m.p2_type} · ${fmtTurns(rounds)} rounds · ${fmtTurns(m.duration_turns)} activations · ${when}`;
      row.addEventListener("click", () => void this.loadReplay(m.id));
      body.appendChild(row);
    }
  }
}

export const spectate = new Spectate();
