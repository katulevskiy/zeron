/// <reference types="@cloudflare/vitest-pool-workers/types" />

// Current Workers pool exposes env as Cloudflare.Env (not ProvidedEnv).
declare namespace Cloudflare {
  interface Env {
    TEST_LOG: DurableObjectNamespace;
    CHAT_ROOMS: DurableObjectNamespace;
    PREVIEW_ROOMS: DurableObjectNamespace;
    REGISTRY_ROOMS: DurableObjectNamespace;
    DEVICE_ROOMS: DurableObjectNamespace;
    BROWSER_SESSIONS: DurableObjectNamespace;
    AUTH_MODE: "dev";
    WORKOS_CLIENT_ID: string;
    WORKOS_API_KEY: string;
    WORKOS_BROWSER_ORIGIN: string;
    WORKOS_ISSUER: string;
    WORKOS_JWKS_URL: string;
    BROWSER_SESSION_KEY: string;
    BROWSER_DEV_ORIGIN: string;
    BROWSER_DEV_OWNER_SUBJECT: string;
    BROWSER_DEV_ORGANIZATION_ID: string;
  }
}
