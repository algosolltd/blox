// Self-check for the backfill handshake. Run with `npm run check`.
//
// Everything else in this package is a DOM write with a formatter in front of
// it. This is the part with real branching: what the adapter does with live
// frames that arrive before the history it is still waiting for.

import assert from "node:assert/strict";
import { FeedAdapter, type ServerMessage, type TradeTick } from "../feed.js";

class FakeSocket {
  static OPEN = 1;
  static CLOSED = 3;
  readyState = FakeSocket.OPEN;
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  sent: string[] = [];
  static last: FakeSocket | null = null;

  constructor(public url: string) {
    FakeSocket.last = this;
  }
  send(data: string) { this.sent.push(data); }
  close() {
    this.readyState = FakeSocket.CLOSED;
    this.onclose?.();
  }
  deliver(msg: ServerMessage) { this.onmessage?.({ data: JSON.stringify(msg) }); }
}

(globalThis as any).WebSocket = FakeSocket;

const tick = (price: number): TradeTick => ({ ts: price, price, qty: 1, side: "B" });
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// ── 1. Live frames that beat the history frame are replayed after it ──────
{
  const feed = new FeedAdapter("ws://x", { historyTimeoutMs: 10_000 });
  const seen: string[] = [];
  feed.subscribe({
    onReset: () => seen.push("reset"),
    onHistory: (t) => seen.push(`history:${t.map((x) => x.price).join(",")}`),
    onTrade: (t) => seen.push(`trade:${t.price}`),
    onBook: (b) => seen.push(`book:${b.bids.length}`),
  });
  feed.connect();
  const ws = FakeSocket.last!;
  ws.onopen!();

  // These arrive while the backfill is still in flight.
  ws.deliver({ type: "update", payload: { trades: [tick(101)], book: { bids: [[1, 1]], asks: [] } } });
  ws.deliver({ type: "update", payload: { trades: [tick(102)] } });
  assert.deepEqual(seen, ["reset"], "live frames must be held until history lands");

  ws.deliver({ type: "history", payload: { trades: [tick(1), tick(2)] } });
  assert.deepEqual(seen, ["reset", "history:1,2", "trade:101", "book:1", "trade:102"],
    "history replays first, then the held frames in arrival order");

  // From here on frames pass straight through.
  ws.deliver({ type: "update", payload: { trades: [tick(103)] } });
  assert.equal(seen[seen.length - 1], "trade:103");
  feed.destroy();
}

// ── 2. A server that never sends history must not wedge the stream ────────
{
  const feed = new FeedAdapter("ws://x", { historyTimeoutMs: 20 });
  const seen: string[] = [];
  feed.subscribe({ onTrade: (t) => seen.push(`trade:${t.price}`), onHistory: () => seen.push("history") });
  feed.connect();
  const ws = FakeSocket.last!;
  ws.onopen!();
  ws.deliver({ type: "update", payload: { trades: [tick(7)] } });
  assert.deepEqual(seen, [], "held while waiting");
  await sleep(40);
  assert.deepEqual(seen, ["trade:7"], "released by the timeout, with no empty history event");
  feed.destroy();
}

// ── 3. destroy() must not schedule a reconnect ────────────────────────────
{
  const feed = new FeedAdapter("ws://x");
  const states: string[] = [];
  feed.subscribe({ onConnection: (s) => states.push(s) });
  feed.connect();
  FakeSocket.last!.onopen!();
  feed.destroy();
  await sleep(20);
  assert.ok(!states.includes("down"), `teardown must not look like a drop: ${states.join(",")}`);
}

// ── 4. A reconnect resets panels and waits for history again ──────────────
{
  const feed = new FeedAdapter("ws://x", { historyTimeoutMs: 10_000 });
  const seen: string[] = [];
  feed.subscribe({ onReset: () => seen.push("reset"), onHistory: () => seen.push("history"), onTrade: () => seen.push("trade") });
  feed.connect();
  const ws = FakeSocket.last!;
  ws.onopen!();
  ws.deliver({ type: "history", payload: { trades: [tick(1)] } });
  ws.deliver({ type: "update", payload: { trades: [tick(2)] } });

  ws.onopen!(); // same socket re-opening stands in for a reconnect
  ws.deliver({ type: "update", payload: { trades: [tick(3)] } });
  assert.deepEqual(seen, ["reset", "history", "trade", "reset"],
    "post-reconnect frames are held again until the new history arrives");
  feed.destroy();
}

console.log("blox-visualize: checks passed");
