import { describe, expect, it } from "vitest";
import { browserPreviewRoute } from "./browser-routes";
import { decodeDeviceFrame, encodeDeviceFrame } from "./device-frame";
import type { Env } from "./env";

const host = `p-${"a".repeat(40)}.preview.test`;
const previewResponse = (header: unknown, body: Uint8Array = new Uint8Array()): Uint8Array => {
  const json = new TextEncoder().encode(JSON.stringify(header));
  const bytes = new Uint8Array(4 + json.length + body.length);
  new DataView(bytes.buffer).setUint32(0, json.length);
  bytes.set(json, 4);
  bytes.set(body, 4 + json.length);
  return bytes;
};
const responseHeader = { status: 200, headers: [["content-type", "text/plain"]] };

async function serve(reply: Uint8Array) {
  const sent: Uint8Array[] = [];
  const events = new EventTarget();
  let closed = false;
  const socket = {
    accept: () => {},
    send: (bytes: Uint8Array) => {
      sent.push(bytes);
      queueMicrotask(() => events.dispatchEvent(new MessageEvent("message", { data: reply.slice().buffer })));
    },
    close: () => { closed = true; },
    addEventListener: events.addEventListener.bind(events)
  };
  const bindings = {
    BROWSER_PREVIEW_ORIGIN: "https://preview.test",
    BROWSER_SESSIONS: {
      idFromName: (name: string) => name,
      get: () => ({ fetch: async () => Response.json({ authenticated: true, parentHash: "parent",
        ownerId: "owner", deviceId: "device", serviceId: "service", host, expiresAt: Date.now() + 60_000 }) })
    },
    DEVICE_ROOMS: {
      idFromName: (name: string) => name,
      get: () => ({ fetch: async () => ({ status: 101, webSocket: socket }) })
    }
  } as unknown as Env;
  const url = new URL(`https://${host}/app?q=1`);
  const response = (await browserPreviewRoute(new Request(url, { headers: { cookie: "__Host-comet_preview=ticket" } }), bindings, url))!;
  expect(closed).toBe(true);
  const outgoing = decodeDeviceFrame(sent[0]!);
  expect(outgoing.header).toMatchObject({ s: expect.any(String), k: "preview" });
  const headerLength = new DataView(outgoing.payload.buffer, outgoing.payload.byteOffset, 4).getUint32(0);
  expect(JSON.parse(new TextDecoder().decode(outgoing.payload.subarray(4, 4 + headerLength)))).toMatchObject({
    service: "service", method: "GET", path: "/app?q=1", headers: []
  });
  return response;
}

describe("browser preview DeviceFrame contract", () => {
  it("uses relay framing and accepts a device header exactly at the 64KiB limit", async () => {
    const header = { s: "stream", k: "preview" };
    const emptySize = new TextEncoder().encode(JSON.stringify(header)).length;
    const exact = { ...header, s: header.s + "x".repeat(64 * 1024 - emptySize) };
    const response = await serve(encodeDeviceFrame(exact, previewResponse(responseHeader, new TextEncoder().encode("preview body"))));
    expect(response.status).toBe(200);
    expect(await response.text()).toBe("preview body");
    expect(response.headers.get("content-security-policy")).toContain("sandbox");
  });

  it.each([
    { name: "oversized device header", bytes: () => encodeDeviceFrame({ s: "x".repeat(64 * 1024), k: "preview" }, previewResponse(responseHeader)) },
    { name: "truncated device header", bytes: () => new Uint8Array([100, 123]) },
    { name: "overlong device length prefix", bytes: () => new Uint8Array([128, 128, 128, 128, 128, 0]) },
    { name: "wrong stream kind", bytes: () => encodeDeviceFrame({ s: "stream", k: "rpc" }, previewResponse(responseHeader)) },
    { name: "invalid preview status", bytes: () => encodeDeviceFrame({ s: "stream", k: "preview" }, previewResponse({ status: 600, headers: [] })) },
    { name: "invalid preview headers", bytes: () => encodeDeviceFrame({ s: "stream", k: "preview" }, previewResponse({ status: 200, headers: {} })) },
    { name: "oversized preview header", bytes: () => encodeDeviceFrame({ s: "stream", k: "preview" }, previewResponse({ ...responseHeader, padding: "x".repeat(64 * 1024) })) }
  ])("rejects $name", async ({ bytes }) => {
    const response = await serve(bytes());
    expect(response.status).toBe(502);
    expect(await response.json()).toEqual({ error: "preview_unavailable" });
  });
});
