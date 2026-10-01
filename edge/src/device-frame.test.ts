import { describe, expect, it } from "vitest";
import { decodeDeviceFrame, encodeDeviceFrame } from "./device-frame";

describe("device frame codec", () => {
  it("matches the native uleb128/JSON/payload wire format", () => {
    const wire = new Uint8Array([19, ...new TextEncoder().encode('{"s":"s","k":"rpc"}'), 7, 8]);
    expect(encodeDeviceFrame({ s: "s", k: "rpc" }, new Uint8Array([7, 8]))).toEqual(wire);
    expect(decodeDeviceFrame(wire)).toEqual({ header: { s: "s", k: "rpc" }, payload: new Uint8Array([7, 8]) });
  });
  it("round-trips header + payload", () => {
    const payload = new Uint8Array([1, 2, 3, 250, 255]);
    const frame = encodeDeviceFrame({ s: "term-42", k: "term", to: "conn-9" }, payload);
    const decoded = decodeDeviceFrame(frame);
    expect(decoded.header).toEqual({ s: "term-42", k: "term", to: "conn-9" });
    expect([...decoded.payload]).toEqual([...payload]);
  });

  it("handles empty payloads and long headers", () => {
    const header = { s: "x".repeat(200), k: "rpc", from: "conn-1" };
    const decoded = decodeDeviceFrame(encodeDeviceFrame(header, new Uint8Array()));
    expect(decoded.header).toEqual(header);
    expect(decoded.payload.length).toBe(0);
  });
});
