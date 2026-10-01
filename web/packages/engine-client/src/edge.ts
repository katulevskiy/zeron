/**
 * The edge Worker's browser API (PR #319's browser-session model): a
 * cookie-backed same-origin session, WorkOS login by redirect, and the
 * owner-scoped device list. No tokens in JS — the HttpOnly session cookie
 * authenticates every route, including the device relay WebSocket.
 */

const JSON_HEADERS = { accept: "application/json" };

/** HTTP failures retain their status so supervision can distinguish expiry from an outage. */
export class BrowserApiError extends Error {
  constructor(readonly status: number, operation: string) {
    super(`${operation} returned HTTP ${status}`);
    this.name = "BrowserApiError";
  }
}

async function browserJson(response: Response, operation: string): Promise<Record<string, unknown>> {
  if (!response.ok) throw new BrowserApiError(response.status, operation);
  if (!response.headers.get("content-type")?.toLowerCase().includes("application/json")) {
    throw new Error("Browser API did not return JSON. Check the same-origin browser backend configuration.");
  }
  const value: unknown = await response.json();
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${operation} returned an invalid JSON object`);
  }
  return value as Record<string, unknown>;
}

export interface BrowserProfile {
  readonly firstName?: string;
  readonly lastName?: string;
  readonly email?: string;
  readonly avatarUrl?: string;
}

export interface BrowserSession {
  readonly authenticated: boolean;
  readonly ownerId?: string;
  readonly organizationId?: string;
  readonly expiresAt?: number;
  readonly csrfToken?: string;
  readonly profile?: BrowserProfile;
}

export interface BrowserDevice {
  readonly id: string;
  readonly name?: string;
  readonly online: boolean;
}

/** The edge session's profile fields, defensively validated. */
function browserProfile(value: unknown): BrowserProfile | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return undefined;
  }
  const source = value as Record<string, unknown>;
  const profile: { firstName?: string; lastName?: string; email?: string; avatarUrl?: string } = {};
  for (const key of ["firstName", "lastName", "email", "avatarUrl"] as const) {
    const item = source[key];
    if (typeof item === "string" && item.length > 0 && item.length <= 2048) {
      profile[key] = item;
    }
  }
  return Object.keys(profile).length > 0 ? profile : undefined;
}

/** `GET /api/browser/session` — the boot probe; never redirects. */
export async function fetchBrowserSession(): Promise<BrowserSession> {
  const response = await fetch("/api/browser/session", {
    headers: JSON_HEADERS,
    credentials: "same-origin",
    cache: "no-store",
  });
  const record = await browserJson(response, "browser session check");
  if (typeof record.authenticated !== "boolean") throw new Error("Invalid browser session response");
  if (record.authenticated && (typeof record.ownerId !== "string" || !record.ownerId || typeof record.csrfToken !== "string" || !record.csrfToken)) {
    throw new Error("Incomplete browser session: owner and CSRF token are required");
  }
  return {
    authenticated: record.authenticated === true,
    ownerId: typeof record.ownerId === "string" ? record.ownerId : undefined,
    organizationId: typeof record.organizationId === "string" ? record.organizationId : undefined,
    expiresAt: typeof record.expiresAt === "number" ? record.expiresAt : undefined,
    csrfToken: typeof record.csrfToken === "string" ? record.csrfToken : undefined,
    profile: browserProfile(record.profile),
  };
}

/**
 * `POST /api/browser/login` — begins the WorkOS AuthKit flow (the edge
 * builds the PKCE authorize URL). The caller does a full-page redirect to
 * it; AuthKit returns through `/api/browser/callback`, which sets the
 * session cookie and redirects back to the app root.
 */
export async function startBrowserLogin(): Promise<string> {
  const response = await fetch("/api/browser/login", {
    method: "POST",
    headers: { ...JSON_HEADERS, "content-type": "application/json" },
    credentials: "same-origin",
    cache: "no-store",
  });
  const record = await browserJson(response, "browser login");
  if (typeof record.authorizationUrl !== "string") {
    throw new Error("browser login response is missing the authorization URL");
  }
  return record.authorizationUrl;
}

/** `GET /api/browser/devices` — the signed-in owner's engines. */
export async function fetchBrowserDevices(): Promise<readonly BrowserDevice[]> {
  const response = await fetch("/api/browser/devices", {
    headers: JSON_HEADERS,
    credentials: "same-origin",
    cache: "no-store",
  });
  const record = await browserJson(response, "browser devices");
  const devices = Array.isArray(record.devices) ? record.devices : [];
  return devices.filter(
    (device): device is BrowserDevice =>
      typeof (device as BrowserDevice | null)?.id === "string" &&
      typeof (device as BrowserDevice).online === "boolean",
  );
}

/** `POST /api/browser/logout` (CSRF-guarded) — revoke and clear the cookie. */
export async function browserLogout(csrfToken: string): Promise<void> {
  if (!csrfToken) throw new Error("Sign-out requires the browser session CSRF token");
  const response = await fetch("/api/browser/logout", {
    method: "POST",
    headers: { ...JSON_HEADERS, "x-csrf-token": csrfToken },
    credentials: "same-origin",
    cache: "no-store",
  });
  if (!response.ok) throw new BrowserApiError(response.status, "browser logout");
}

/** Only genuine foreground interaction calls this; polling must never extend idle life. */
export async function browserActivity(csrfToken: string): Promise<void> {
  if (!csrfToken) throw new Error("Browser activity requires the session CSRF token");
  const response = await fetch("/api/browser/activity", {
    method: "POST",
    headers: { ...JSON_HEADERS, "x-csrf-token": csrfToken },
    credentials: "same-origin",
    cache: "no-store",
  });
  const record = await browserJson(response, "browser activity");
  if (record.authenticated !== true) throw new BrowserApiError(401, "browser activity");
}

/**
 * The device relay WebSocket: same-origin, the browser's session cookie
 * authenticates the upgrade. Canonical device IDs include underscores.
 */
export function relayDeviceUrl(deviceId: string): string {
  if (!/^[A-Za-z0-9_-]{1,128}$/.test(deviceId)) {
    throw new Error("Invalid remote device id");
  }
  const protocol = window.location.protocol === "https:" ? "wss" : "ws";
  return `${protocol}://${window.location.host}/api/browser/device/${deviceId}/ws`;
}
