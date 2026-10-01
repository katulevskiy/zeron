import { WebSocket as NodeWebSocket } from "ws";
import { vi } from "vitest";
import { RelaySocket } from "../../src/device-frame";
import type { WebSocketFactory } from "../../src/socket";
import type { BrowserRelaySession } from "./browser-relay";

/** Browser cookie transport in Node; production RelaySocket handles all framing/liveness. */
export function trackedCookieRelay(session: BrowserRelaySession): {
  factory: WebSocketFactory;
  sockets: NodeWebSocket[];
} {
  const sockets: NodeWebSocket[] = [];
  const origin = new URL(session.relayUrl("unused"));
  class CookieWebSocket extends NodeWebSocket {
    constructor(url: string) {
      if (new URL(url).origin !== origin.origin) {
        throw new Error("test session cookie cannot leave its isolated relay origin");
      }
      super(url, { headers: { Cookie: session.cookie, Origin: origin.origin.replace(/^ws/, "http") } });
      sockets.push(this);
    }
  }
  // Only the platform socket is supplied by Node. Neither the product RPC
  // client, RelaySocket nor the real edge/engine are replaced or scripted.
  return {
    sockets,
    factory: url => {
      vi.stubGlobal("WebSocket", CookieWebSocket);
      return new RelaySocket(url);
    },
  };
}
