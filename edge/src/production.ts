import edge from "./index";
import type { Env } from "./env";

// Preserve exported classes and existing native DO identities.
export { SessionRoom, DeviceRoom, RegistryRoom, ChatRoom, PreviewRoom, BrowserSessionStore } from "./index";
type ProductionEnv = Env & { ASSETS: Fetcher };

/** React/Vite candidate entry point. The native deployment still uses index.ts. */
export default {
  async fetch(request: Request, env: ProductionEnv): Promise<Response> {
    const url = new URL(request.url);
    // All APIs, native routes and upgrades precede SPA fallback. An explicit
    // origin is required even for assets; a typo must not create an app origin.
    const appOrigin = env.AUTH_MODE === "dev" ? env.BROWSER_DEV_ORIGIN : env.WORKOS_BROWSER_ORIGIN;
    const document = request.headers.get("accept")?.includes("text/html");
    const reserved = /^\/(?:api|auth|device|session|workspace|registry|chat2|preview|blob|attachments|releases|tail|stats|diff|snapshot|append)(?:\/|$)/.test(url.pathname)
      || url.pathname === "/health" || url.pathname === "/install.sh";
    if (url.origin !== appOrigin || reserved || request.headers.has("upgrade")
      || (request.method !== "GET" && request.method !== "HEAD")) return edge.fetch(request, env);
    const publicAsset = ["/assets/", "/backgrounds/", "/file-icons/", "/sounds/"].some((prefix) => url.pathname.startsWith(prefix))
      || ["/favicon.ico", "/favicon.svg", "/favicon.png", "/apple-touch-icon.png"].includes(url.pathname);
    if (publicAsset) return env.ASSETS.fetch(request);
    if (url.pathname === "/" || url.pathname === "/index.html" || (document && !url.pathname.split("/").at(-1)?.includes("."))) {
      url.pathname = "/index.html";
      url.search = "";
      return env.ASSETS.fetch(new Request(url, request));
    }
    return edge.fetch(request, env);
  }
};
