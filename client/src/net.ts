// Minimal WebSocket wrapper with typed callbacks.

import type { ClientMsg, ServerMsg } from "./types";

export class Net {
  private ws: WebSocket | null = null;
  private queue: ClientMsg[] = [];

  connect(onMessage: (msg: ServerMsg) => void, onClose: () => void): void {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${location.host}/ws`);
    this.ws = ws;
    let notified = false;
    const notifyClosed = (): void => {
      if (notified || this.ws !== ws) return;
      notified = true;
      this.ws = null;
      onClose();
    };
    ws.onopen = () => {
      if (this.ws !== ws) return;
      for (const m of this.queue) ws.send(JSON.stringify(m));
      this.queue = [];
    };
    ws.onmessage = (ev) => {
      if (this.ws !== ws) return;
      try {
        const msg = JSON.parse(ev.data as string) as ServerMsg;
        onMessage(msg);
      } catch (e) {
        // One malformed frame must not kill the session: log and skip it.
        console.error("dropping unparseable server message", e, ev.data);
      }
    };
    ws.onclose = notifyClosed;
    ws.onerror = notifyClosed;
  }

  send(msg: ClientMsg): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    } else {
      this.queue.push(msg);
    }
  }

  close(): void {
    const ws = this.ws;
    this.ws = null;
    ws?.close();
  }
}
