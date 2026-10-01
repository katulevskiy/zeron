/**
 * Edge-issued device tokens (docs/cloud.md §6): short-lived ES256 JWTs a
 * cloud box gets from `POST /auth/device-token` in exchange for its device
 * credential. Claims: `iss` "zeron-edge", `sub` = owner user id, `org_id`,
 * `did` = the box's device id, `kind` "cloud", `iat`/`exp` (30 minutes).
 *
 * The signing key is the wrangler secret `ZERON_DEVICE_JWT_KEY`: a private
 * P-256 key as a JWK (JSON) or PKCS8 PEM. Its public half is published at
 * `/.well-known/zeron-device-jwks.json`. Unset ⇒ device tokens are neither
 * issued nor accepted.
 */
import {
  type CryptoKey,
  type JWK,
  SignJWT,
  calculateJwkThumbprint,
  decodeJwt,
  exportJWK,
  importJWK,
  importPKCS8,
  jwtVerify
} from "jose";

export const DEVICE_TOKEN_ISSUER = "zeron-edge";
export const DEVICE_TOKEN_KIND = "cloud";
export const DEVICE_TOKEN_TTL_S = 30 * 60;

export interface DeviceKeyEnv {
  ZERON_DEVICE_JWT_KEY?: string;
}

interface DeviceKeys {
  kid: string;
  privateKey: CryptoKey;
  publicKey: CryptoKey;
  publicJwk: JWK;
}

const keyCache = new Map<string, Promise<DeviceKeys>>();

const loadKeys = async (secret: string): Promise<DeviceKeys> => {
  const trimmed = secret.trim();
  let privateJwk: JWK;
  if (trimmed.startsWith("{")) {
    privateJwk = JSON.parse(trimmed) as JWK;
  } else {
    const imported = await importPKCS8(trimmed, "ES256", { extractable: true });
    privateJwk = await exportJWK(imported);
  }
  if (privateJwk.kty !== "EC" || privateJwk.crv !== "P-256" || !privateJwk.d) {
    throw new Error("ZERON_DEVICE_JWT_KEY must be a private P-256 key");
  }
  const publicJwk: JWK = { kty: "EC", crv: "P-256", x: privateJwk.x, y: privateJwk.y };
  const kid = privateJwk.kid ?? (await calculateJwkThumbprint(publicJwk));
  return {
    kid,
    privateKey: (await importJWK({ ...privateJwk, alg: "ES256" }, "ES256")) as CryptoKey,
    publicKey: (await importJWK({ ...publicJwk, alg: "ES256" }, "ES256")) as CryptoKey,
    publicJwk: { ...publicJwk, kid, alg: "ES256", use: "sig" }
  };
};

const deviceKeys = (env: DeviceKeyEnv): Promise<DeviceKeys> | undefined => {
  const secret = env.ZERON_DEVICE_JWT_KEY;
  if (!secret) return undefined;
  let keys = keyCache.get(secret);
  if (!keys) {
    keys = loadKeys(secret);
    // A malformed secret must not poison the isolate forever.
    keys.catch(() => keyCache.delete(secret));
    keyCache.set(secret, keys);
  }
  return keys;
};

export const deviceTokensConfigured = (env: DeviceKeyEnv): boolean => !!env.ZERON_DEVICE_JWT_KEY;

export interface DeviceTokenClaims {
  userId: string;
  orgId: string;
  deviceId: string;
}

export const signDeviceToken = async (
  env: DeviceKeyEnv,
  claims: DeviceTokenClaims,
  nowMs = Date.now()
): Promise<{ token: string; expiresAt: number }> => {
  const keys = await deviceKeys(env);
  if (!keys) throw new Error("device tokens are not configured");
  const iat = Math.floor(nowMs / 1000);
  const exp = iat + DEVICE_TOKEN_TTL_S;
  const token = await new SignJWT({
    org_id: claims.orgId,
    did: claims.deviceId,
    kind: DEVICE_TOKEN_KIND
  })
    .setProtectedHeader({ alg: "ES256", kid: keys.kid, typ: "JWT" })
    .setIssuer(DEVICE_TOKEN_ISSUER)
    .setSubject(claims.userId)
    .setIssuedAt(iat)
    .setExpirationTime(exp)
    .sign(keys.privateKey);
  return { token, expiresAt: exp * 1000 };
};

/** Whether `token` claims to be ours (unverified `iss`). Such tokens are
 * verified only against the device key — never handed to WorkOS or dev
 * parsing — so a forged one fails closed. */
export const isDeviceToken = (token: string): boolean => {
  try {
    return decodeJwt(token).iss === DEVICE_TOKEN_ISSUER;
  } catch {
    return false;
  }
};

/** Verify a device token. `undefined` = invalid, expired, or not configured. */
export const verifyDeviceToken = async (
  env: DeviceKeyEnv,
  token: string
): Promise<DeviceTokenClaims | undefined> => {
  try {
    const keys = await deviceKeys(env);
    if (!keys) return undefined;
    const { payload } = await jwtVerify(token, keys.publicKey, {
      issuer: DEVICE_TOKEN_ISSUER,
      algorithms: ["ES256"],
      requiredClaims: ["sub", "exp", "iat"]
    });
    if (
      payload.kind !== DEVICE_TOKEN_KIND ||
      typeof payload.sub !== "string" ||
      payload.sub.length === 0 ||
      typeof payload.org_id !== "string" ||
      typeof payload.did !== "string"
    ) {
      return undefined;
    }
    return { userId: payload.sub, orgId: payload.org_id, deviceId: payload.did };
  } catch {
    return undefined;
  }
};

export const deviceJwks = async (env: DeviceKeyEnv): Promise<{ keys: JWK[] }> => {
  const keys = deviceKeys(env);
  return { keys: keys ? [(await keys).publicJwk] : [] };
};
