import express from "express";
import http from "http";
import path from "path";
import { WebSocketServer, WebSocket } from "ws";
import { createServer as createViteServer } from "vite";

import { store } from "./server/store";
import { GameMatch } from "./server/sim";
import { createBot, type Bot } from "./server/bot";
import { DEFAULT_TERRAIN_RULES, type ClientMsg } from "./client/src/types";

const PORT = 3000;
const app = express();
app.use(express.json());

// REST API Endpoints
app.get("/api/hello", (_req, res) => {
  res.json({ service: "crucible-server", sim: "0.1.0" });
});

app.get("/api/health", (_req, res) => {
  res.json({
    ok: true,
    uptime_secs: Math.floor((Date.now() - store.startedAt) / 1000),
  });
});

app.get("/api/status", (_req, res) => {
  res.json({
    ok: true,
    uptime_secs: Math.floor((Date.now() - store.startedAt) / 1000),
    counts: {
      matches: store.replays.length,
      genomes: 45,
      champions: store.champions.length,
      events: store.recentEvents.length,
    },
    recent_events: store.recentEvents,
    trainer: {
      running: true,
      generation: 4,
      matches_run: 250,
    },
  });
});

app.get("/api/champion", (_req, res) => {
  res.json({ champion: store.getReigningChampion() });
});

app.get("/api/museum", (_req, res) => {
  res.json({ champions: store.getMuseum() });
});

app.get("/api/training-stats", (_req, res) => {
  res.json({ stats: store.trainingStats });
});

app.get("/api/elo-history", (_req, res) => {
  res.json({ points: store.eloHistory });
});

app.get("/api/lineage/:id", (req, res) => {
  const genomeId = parseInt(req.params.id, 10) || 104;
  res.json({ lineage: store.getLineage(genomeId) });
});

app.get("/api/replays", (_req, res) => {
  res.json({ matches: store.listReplays() });
});

app.get("/api/replay/:id", (req, res) => {
  const id = parseInt(req.params.id, 10);
  const replayData = store.getReplay(id);
  if (!replayData) {
    res.status(404).json({ error: "Replay not found" });
    return;
  }
  res.json({ replay: replayData });
});

app.post("/api/report/:old/:new", (_req, res) => {
  res.json({ ok: true });
});

app.post("/api/autobattle/:a/:b", (_req, res) => {
  res.json({ ok: true, result: "Draw" });
});

// Create HTTP server
const server = http.createServer(app);

// WebSocket Server
const wss = new WebSocketServer({ server, path: "/ws" });

wss.on("connection", (ws: WebSocket) => {
  let game: GameMatch | null = null;
  let bot: Bot | null = null;
  let opponentName = "medium";

  ws.on("message", (data: string | Buffer) => {
    try {
      const msg = JSON.parse(data.toString()) as ClientMsg;

      if (msg.type === "joinMatch") {
        opponentName = msg.opponent || "medium";
        bot = createBot(opponentName);
        game = new GameMatch();

        // Send MatchStart
        ws.send(
          JSON.stringify({
            type: "matchStart",
            mapSeed: game.map.seed,
            player: 0,
            passable: game.map.passable,
            terrain: game.map.terrain,
            terrainRules: DEFAULT_TERRAIN_RULES,
            elevation: game.map.elevation,
            moisture: game.map.moisture,
            temperature: game.map.temperature,
            hq: game.map.hq,
          })
        );

        // Send initial StateDiff
        ws.send(JSON.stringify(game.getDiffForP0()));
      } else if (msg.type === "commands" && game) {
        game.applyCommands(0, msg.cmds);
        ws.send(JSON.stringify(game.getDiffForP0()));
      } else if (msg.type === "inspectTile" && game) {
        const tile = game.getTileInspection(msg.x, msg.y);
        ws.send(JSON.stringify({ type: "tileInspection", ...tile }));
      } else if (msg.type === "endTurn" && game) {
        // Human ends turn
        game.endHumanTurn();

        // Bot acts
        if (bot) {
          bot.act(game);
        }
        game.endBotTurn();

        // Check victory
        if (game.winner !== null) {
          const repId = store.addReplay(
            {
              id: 0,
              map_seed: game.map.seed,
              p1_type: "Human Commander",
              p2_type: bot?.name || "AI",
              result: game.winner === 0 ? "P0 Victory" : "Bot Victory",
              duration_turns: game.turn,
              duration_rounds: game.round,
              created_at: Date.now(),
            },
            JSON.stringify({
              meta: {
                map_seed: game.map.seed,
                passable: game.map.passable,
                terrain: game.map.terrain,
                hq_tiles: game.map.hq,
                ore: Array.from({ length: 16384 }, () => 0),
                crystal: Array.from({ length: 16384 }, () => 0),
                duration_turns: game.turn,
                duration_rounds: game.round,
                winner: game.winner,
                win_reason: game.winReason,
              },
              frames: [],
            })
          );

          ws.send(
            JSON.stringify({
              type: "matchEnd",
              winner: game.winner,
              reason: game.winReason,
              durationTurns: game.turn,
              durationRounds: game.round,
              replayId: repId,
            })
          );
        }

        // Send updated state
        ws.send(JSON.stringify(game.getDiffForP0()));
      }
    } catch (err) {
      console.error("Error processing websocket message:", err);
    }
  });
});

async function start() {
  if (process.env.NODE_ENV !== "production") {
    const vite = await createViteServer({
      server: { middlewareMode: true },
      appType: "spa",
      configFile: path.resolve(process.cwd(), "vite.config.ts"),
    });
    app.use(vite.middlewares);
  } else {
    const distPath = path.resolve(process.cwd(), "dist");
    app.use(express.static(distPath));
    app.get("*all", (_req, res) => {
      res.sendFile(path.join(distPath, "index.html"));
    });
  }

  server.listen(PORT, "0.0.0.0", () => {
    console.log(`CRUCIBLE server listening on http://0.0.0.0:${PORT}`);
  });
}

start();
