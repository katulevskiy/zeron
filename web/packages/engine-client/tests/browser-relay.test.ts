import { once } from "node:events";
import { access } from "node:fs/promises";
import { dirname, join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { WebSocket } from "ws";
import type { EngineInfo } from "@zeron/proto";
import { decodeServerMessage, encodeClientFrame } from "../src/codec";
import { decodeDeviceFrame, encodeDeviceFrame, RPC_KIND } from "../src/device-frame";
import { ENGINE_INFO } from "../src/methods";
import { startBrowserRelayFixture, type BrowserRelayEngine, type BrowserRelayFixture, type BrowserRelaySession } from "./helpers/browser-relay";

/** Ordinary wire client using the product codecs, not a replacement relay/server. */
async function engineInfo(session: BrowserRelaySession, engine: BrowserRelayEngine): Promise<EngineInfo> {
  const socket = session.openSocket(engine.deviceId);
  try {
    const response = new Promise<EngineInfo>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("real relay EngineInfo timed out")), 10_000);
      socket.on("error", error => { clearTimeout(timer); reject(error); });
      socket.on("close", (code, reason) => { clearTimeout(timer); reject(new Error(`relay closed ${code}: ${reason}`)); });
      socket.on("message", (data, binary) => {
        try {
          expect(binary).toBe(true);
          const bytes = data instanceof ArrayBuffer ? new Uint8Array(data) : new Uint8Array(Buffer.concat(Array.isArray(data) ? data : [data]));
          const { header, payload } = decodeDeviceFrame(bytes);
          if (header.k !== RPC_KIND) throw new Error(`relay control frame ${header.k}`);
          const decoded = decodeServerMessage(new TextDecoder().decode(payload));
          expect(decoded.malformed).toBe(0);
          for (const frame of decoded.frames) {
            if (frame.id !== 1) continue;
            clearTimeout(timer);
            if (frame.err) reject(new Error(frame.err));
            else if (frame.ok) resolve(frame.ok as EngineInfo);
          }
        } catch (error) { clearTimeout(timer); reject(error); }
      });
    });
    // Attach rejection handling while the socket is opening, so refused upgrades
    // cannot produce an unhandled promise rejection in the test runner.
    const opened = once(socket, "open");
    await Promise.race([opened, response]);
    socket.send(encodeDeviceFrame({ s: RPC_KIND, k: RPC_KIND }, new TextEncoder().encode(encodeClientFrame({ id: 1, method: ENGINE_INFO }))));
    return await response;
  } finally {
    socket.terminate();
  }
}

let fixture: BrowserRelayFixture | undefined;
beforeEach(async () => { fixture = await startBrowserRelayFixture(); });
afterEach(async () => { await fixture?.stop(); });

describe("cookie browser relay through actual edge/workerd and EngineCore", () => {
  test("real HttpOnly session discovers two registered hosts and EngineInfo crosses the relay", async () => {
    const session = fixture!.session;
    const response = await session.request("/api/browser/session");
    expect(response.status).toBe(200);
    expect(await response.json()).toMatchObject({ authenticated: true, ownerId: session.ownerId, organizationId: session.organizationId });
    expect(session.browserCookies()[0]).toMatchObject({ httpOnly: true, sameSite: "Strict" });
    const origin = new URL(fixture!.origin);
    expect(origin.hostname).toBe("127.0.0.1");
    expect(origin.port).toBe(fixture!.edgeChild.spawnargs[fixture!.edgeChild.spawnargs.indexOf("--port") + 1]);
    expect(new Set(fixture!.engines.map(engine => engine.deviceId)).size).toBe(2);
    for (const engine of fixture!.engines) {
      const info = await engineInfo(session, engine);
      expect(info.deviceId).toBe(engine.deviceId);
      expect(info.workspaceScope).toBe("development");
      expect(info.capabilities).not.toContain("web-client");
      expect(info.capabilities).toContain("message-queue-v1");
      console.log(`Real cookie relay ${fixture!.origin} -> ${engine.deviceId} pid=${engine.child.pid}`);
    }
  });

  test("missing browser cookie refuses HTTP discovery and WebSocket upgrade", async () => {
    expect((await fetch(`${fixture!.origin}/api/browser/devices`)).status).toBe(401);
    const socket = new WebSocket(fixture!.session.relayUrl(fixture!.engines[0]!.deviceId), {
      headers: { Origin: fixture!.origin }, handshakeTimeout: 5000,
    });
    try { await expect(once(socket, "open")).rejects.toThrow(/401/); }
    finally { socket.terminate(); }
  });
  test("host disconnect/reconnect and process restart preserve real identity", async () => {
    const engine = fixture!.engines[0]!;
    const session = fixture!.session;
    const identity = engine.deviceId;
    await engine.disconnect();
    await fixture!.waitForDevice(session, engine, false);
    await engine.reconnect();
    await fixture!.waitForDevice(session, engine);
    expect((await engineInfo(session, engine)).deviceId).toBe(identity);
    const oldPid = engine.child.pid!;
    await engine.stop();
    expect(() => process.kill(oldPid, 0)).toThrow(/ESRCH/);
    await fixture!.waitForDevice(session, engine, false);
    await engine.restart();
    await fixture!.waitForDevice(session, engine);
    expect((await engineInfo(session, engine)).deviceId).toBe(identity);
  });

  test("real cookie accounts retain separate device ownership across a loopback login switch", async () => {
    const accountA = fixture!.session;
    const ownA = fixture!.engines.filter(engine => engine.ownerId === accountA.ownerId);
    const other = await fixture!.addEngine({ label: "owner-b-engine", ownerId: "relay-owner-b" });
    const accountB = await fixture!.loginAs(other.ownerId);
    await fixture!.waitForDevice(accountB, other);
    const devicesB = await (await accountB.request("/api/browser/devices")).json() as { devices: Array<{ id: string }> };
    expect(devicesB.devices.map(device => device.id)).toEqual([other.deviceId]);
    expect((await engineInfo(accountB, other)).deviceId).toBe(other.deviceId);
    await expect(engineInfo(accountB, ownA[0]!)).rejects.toThrow(/403/);
    const devicesA = await (await accountA.request("/api/browser/devices")).json() as { devices: Array<{ id: string }> };
    expect(devicesA.devices.map(device => device.id).sort()).toEqual(ownA.map(engine => engine.deviceId).sort());
    await expect(engineInfo(accountA, other)).rejects.toThrow(/403/);
  });

  test("failed real-engine startup preserves diagnostics and removes its isolated worker/storage", async () => {
    const error = await startBrowserRelayFixture({ ownerId: "invalid@fixture", engineLabels: ["fault-engine"] }).catch(error => error as Error);
    expect(error).toBeInstanceOf(Error);
    expect((error as Error).message).toMatch(/invalid fixture identity/);
    const directory = (error as Error).message.match(/browser-relay-[A-Za-z0-9]{6}/)?.[0];
    expect(directory).toBeDefined();
    await expect(access(join(dirname(fixture!.root), directory!))).rejects.toThrow(/ENOENT/);
    // Failure cleanup must not interfere with the independent, healthy fixture.
    expect(() => process.kill(fixture!.engines[0]!.child.pid!, 0)).not.toThrow();
  });
  test("normal or failed test-body cleanup releases processes, endpoint and owned storage", async () => {
    const root = fixture!.root;
    const pids = [fixture!.edgeChild.pid!, ...fixture!.engines.map(engine => engine.child.pid!)];
    const origin = fixture!.origin;
    try { throw new Error("test-owned failure"); }
    catch (error) { expect((error as Error).message).toBe("test-owned failure"); }
    finally { await fixture!.stop(); }
    await fixture!.stop();
    for (const pid of pids) expect(() => process.kill(pid, 0)).toThrow(/ESRCH/);
    await expect(fetch(`${origin}/health`, { signal: AbortSignal.timeout(1000) })).rejects.toThrow();
    await expect(access(root)).rejects.toThrow(/ENOENT/);
  });
});
