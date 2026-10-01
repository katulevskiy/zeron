import { BytesReader, BytesWriter } from "loro-protocol";

/** Binary relay framing: uleb128 header-length ‖ UTF-8 JSON header ‖ payload. */
export interface DeviceFrameHeader {
  /** Stream id, unique per (connId, logical stream). */
  s: string;
  /** Stream kind: "rpc" | "term" | ... — opaque to the relay. */
  k: string;
  /** Routing: host→client target. */
  to?: string;
  /** Routing: client→host origin (stamped by the relay). */
  from?: string;
}

export const encodeDeviceFrame = (header: DeviceFrameHeader, payload: Uint8Array): Uint8Array => {
  const writer = new BytesWriter();
  writer.pushVarString(JSON.stringify(header));
  writer.pushBytes(payload);
  return writer.finalize();
};

export const decodeDeviceFrame = (
  bytes: Uint8Array,
  maxHeaderBytes?: number
): { header: DeviceFrameHeader; payload: Uint8Array } => {
  const reader = new BytesReader(bytes);
  const size = reader.readUleb128();
  // Preview callers keep their 64KiB bound and five-byte length-prefix limit.
  // Check before UTF-8 decoding/JSON parsing; the relay's unbounded codec stays unchanged.
  if (maxHeaderBytes !== undefined && (reader.position > 5 || size > maxHeaderBytes)) {
    throw new Error("device frame header too large");
  }
  const header = JSON.parse(new TextDecoder().decode(reader.readBytes(size))) as DeviceFrameHeader;
  const payload = reader.readBytes(reader.remaining);
  return { header, payload };
};
