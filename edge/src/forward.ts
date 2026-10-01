import type { Verified } from "./auth";
import { AUTH_DEVICE_HEADER, AUTH_USER_HEADER, ROOM_KIND_HEADER } from "./env";

/** Forward into a DO with the verified identity stamped on the request. */
export const forward = (
  ns: DurableObjectNamespace,
  name: string,
  request: Request,
  auth: Verified,
  path: string,
  search?: string,
  roomKind?: "workspace"
): Promise<Response> => {
  const stub = ns.get(ns.idFromName(name));
  const url = new URL(request.url);
  url.pathname = path;
  if (search !== undefined) url.search = search;
  const headers = new Headers(request.headers);
  // room-kind is a Worker-controlled signal (the DO relaxes owner gating for
  // workspace rooms): clear any inbound value so only the explicit set below —
  // reached solely on workspace forwards, after the org-membership check —
  // can assert it. Do not drop this line; passthrough would let a caller
  // choose their own room kind. The device header is cleared for the same
  // reason: only a verified device token may assert it.
  headers.delete(ROOM_KIND_HEADER);
  headers.delete(AUTH_DEVICE_HEADER);
  headers.set(AUTH_USER_HEADER, auth.userId);
  if (auth.deviceId) headers.set(AUTH_DEVICE_HEADER, auth.deviceId);
  if (roomKind) headers.set(ROOM_KIND_HEADER, roomKind);
  return stub.fetch(
    new Request(url.toString(), { method: request.method, body: request.body, headers })
  );
};
