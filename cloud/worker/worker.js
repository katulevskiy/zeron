// Zeron cloud box Worker (docs/cloud.md §2–§5).
//
// Uploaded as-is into the user's own Cloudflare account by the desktop
// engine (crates/cloud), together with core.js. No build step, no
// dependencies.
//
//   POST /v1/boxes/{deviceId}/wake|stop|status|purge   signed with the box's wakeKey
//   ZeronBox    one Durable Object per box: starts and stops its container
//   R2Gateway   http://r2.zeron.internal inside the container → R2, confined
//               to boxes/{deviceId}/

import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";
import {
  ALARM_MS,
  CHECKPOINT_HOST,
  CONTROL_PORT,
  DEFAULT_IDLE_MINUTES,
  FINAL_CHECKPOINT_MS,
  SIGTERM,
  decideAlarm,
  handleCheckpoint,
  idleMs,
  json,
  parseBoxes,
  purge,
  randomToken,
  route,
  verifySignature,
} from "./core.js";

const TIMESTAMP_HEADER = "x-zeron-timestamp";
const SIGNATURE_HEADER = "x-zeron-signature";

// Only entrypoints are exported from this module; helpers live in core.js.

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const target = route(request.method, url.pathname);
    if (!target) return json({ error: "not found" }, 404);
    if (target.status) return json({ error: "method not allowed" }, target.status);
    const entry = parseBoxes(env.ZERON_BOXES)[target.deviceId];
    if (!entry?.wakeKey) return json({ error: "unknown box" }, 404);
    const body = await request.arrayBuffer();
    if (body.byteLength > 4096) return json({ error: "body too large" }, 413);
    const problem = await verifySignature({
      wakeKey: entry.wakeKey,
      method: request.method,
      path: url.pathname,
      timestamp: request.headers.get(TIMESTAMP_HEADER),
      signature: request.headers.get(SIGNATURE_HEADER),
      body,
      now: Date.now(),
    });
    if (problem) return json({ error: problem }, 401);

    if (target.action === "purge") {
      return json(await purge(env.CKPT, target.deviceId));
    }
    const box = env.BOX.getByName(target.deviceId);
    const spec = {
      deviceId: target.deviceId,
      credential: entry.credential,
      idleMinutes: Number(entry.idleMinutes) || DEFAULT_IDLE_MINUTES,
    };
    try {
      if (target.action === "wake") return json(await box.wake(spec));
      if (target.action === "stop") return json(await box.stop());
      return json(await box.status());
    } catch (error) {
      return json({ error: String(error?.message ?? error) }, 500);
    }
  },
};

// ── the box (§3) ─────────────────────────────────────────────────────────────

/** One per box (`getByName(deviceId)`); its state lives in DO storage under `box`. */
export class ZeronBox extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    // After a Durable Object restart the container may still be running:
    // set its timeout and checkpoint route again.
    ctx.blockConcurrencyWhile(async () => {
      if (!ctx.container?.running) return;
      await this.arm(await this.load());
    });
  }

  async load() {
    return (await this.ctx.storage.get("box")) ?? { state: "asleep", stateAt: 0 };
  }

  async save(box) {
    await this.ctx.storage.put("box", box);
  }

  async arm(box) {
    const container = this.ctx.container;
    await container.setInactivityTimeout(idleMs(box));
    if (box.deviceId) {
      const gateway = this.ctx.exports.R2Gateway({ props: { deviceId: box.deviceId } });
      await container.interceptOutboundHttp(CHECKPOINT_HOST, gateway);
    }
  }

  async wake({ deviceId, credential, idleMinutes }) {
    const container = this.ctx.container;
    let box = { ...(await this.load()), deviceId, idleMinutes };
    const now = Date.now();
    if (container.running) {
      if (box.state === "asleep" || box.state === "error") {
        box = { ...box, state: "waking", stateAt: now, error: null };
      }
      await this.arm(box);
      await this.save(box);
      if (!(await this.ctx.storage.getAlarm())) await this.ctx.storage.setAlarm(now + ALARM_MS);
      return { state: box.state === "running" ? "running" : "waking" };
    }
    const controlToken = randomToken();
    container.start({
      enableInternet: true,
      env: {
        ZERON_DEVICE_ID: deviceId,
        ZERON_DEVICE_CREDENTIAL: credential ?? "",
        ZERON_EDGE_URL: this.env.ZERON_EDGE_URL ?? "",
        ZERON_CONTROL_TOKEN: controlToken,
        ZERON_CLOUD_CONTROL_PORT: String(CONTROL_PORT),
        ZERON_CHECKPOINT_URL: `http://${CHECKPOINT_HOST}`,
      },
    });
    box = {
      ...box,
      state: "waking",
      stateAt: now,
      startedAt: now,
      controlToken,
      lastOkAt: null,
      lastActivityAt: null,
      busy: false,
      stopRequested: false,
      error: null,
    };
    await this.save(box);
    await this.arm(box);
    this.watch();
    await this.ctx.storage.setAlarm(now + ALARM_MS);
    return { state: "waking" };
  }

  /** Record the container's exit while this instance is alive (the alarm and `status` cover the rest). */
  watch() {
    this.ctx.container
      .monitor()
      .then(() => this.stopped(null))
      .catch((error) => this.stopped(String(error?.message ?? error)));
  }

  async stopped(error) {
    const box = await this.load();
    await this.save({
      ...box,
      state: error ? "error" : "asleep",
      stateAt: Date.now(),
      error,
      stopRequested: false,
    });
  }

  /** Checkpoint and stop on the next alarm (now): the alarm has the time a final checkpoint needs. */
  async stop() {
    const box = await this.load();
    if (!this.ctx.container.running) {
      const asleep = { ...box, state: "asleep", stateAt: Date.now(), stopRequested: false };
      await this.save(asleep);
      return { state: "asleep" };
    }
    await this.save({ ...box, state: "checkpointing", stateAt: Date.now(), stopRequested: true });
    await this.ctx.storage.setAlarm(Date.now());
    return { state: "checkpointing" };
  }

  async status() {
    let box = await this.load();
    const running = this.ctx.container.running;
    if (!running && box.state !== "asleep" && box.state !== "error") {
      box = { ...box, state: "asleep", stateAt: Date.now() };
      await this.save(box);
    } else if (running && (box.state === "waking" || box.state === "running")) {
      const status = await this.control(box, "GET", "/zeron/cloud/status", 3_000).catch(() => null);
      if (status) {
        box = this.observed(box, status, Date.now());
        await this.save(box);
      }
    }
    return {
      state: box.state,
      startedAt: box.startedAt ?? undefined,
      lastActivityAt: box.lastActivityAt ?? undefined,
      busy: running ? Boolean(box.busy) : false,
    };
  }

  observed(box, status, now) {
    return {
      ...box,
      state: box.state === "waking" ? "running" : box.state,
      stateAt: box.state === "waking" ? now : box.stateAt,
      lastOkAt: now,
      busy: Boolean(status.busy),
      lastActivityAt: Number(status.lastActivityAt) || box.lastActivityAt || null,
    };
  }

  async alarm() {
    let box = await this.load();
    const container = this.ctx.container;
    const now = Date.now();
    if (!container.running) {
      if (box.state !== "error") box = { ...box, state: "asleep", stateAt: now };
      await this.save({ ...box, stopRequested: false });
      return;
    }
    let decision = "sleep";
    if (!box.stopRequested) {
      const status = await this.control(box, "GET", "/zeron/cloud/status", 10_000).catch(() => null);
      if (status) box = this.observed(box, status, now);
      decision = decideAlarm({
        running: true,
        status,
        now,
        idleMs: idleMs(box),
        startedAt: box.startedAt,
        lastOkAt: box.lastOkAt,
      });
    }
    if (decision === "renew") {
      await container.setInactivityTimeout(idleMs(box));
      await this.save(box);
      await this.ctx.storage.setAlarm(now + ALARM_MS);
      return;
    }
    await this.save({ ...box, state: "checkpointing", stateAt: now });
    await this.control(box, "POST", "/zeron/cloud/checkpoint?final=1", FINAL_CHECKPOINT_MS).catch(
      () => null,
    );
    try {
      container.signal(SIGTERM);
    } catch {
      // Already gone.
    }
    await this.save({ ...box, state: "asleep", stateAt: Date.now(), stopRequested: false });
  }

  async control(box, method, path, timeoutMs) {
    const port = this.ctx.container.getTcpPort(CONTROL_PORT);
    const response = await port.fetch(`http://container${path}`, {
      method,
      headers: { "x-zeron-control": box.controlToken ?? "" },
      signal: AbortSignal.timeout(timeoutMs),
    });
    if (!response.ok) throw new Error(`${path}: ${response.status}`);
    return response.json();
  }
}

// ── checkpoint gateway (§5) ──────────────────────────────────────────────────

/** `http://r2.zeron.internal` inside a box's container (`ctx.exports.R2Gateway({props: {deviceId}})`). */
export class R2Gateway extends WorkerEntrypoint {
  fetch(request) {
    return handleCheckpoint(request, this.env.CKPT, this.ctx.props?.deviceId);
  }
}
