import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { WebSocket } from "ws";
import { runCommand, stopChild, waitForReady } from "./engine";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../..");
const edgeRoot = join(repoRoot, "edge");

/** A real subprocess with retained diagnostics and command acknowledgements. */
class FixtureProcess {
  readonly child: ChildProcessWithoutNullStreams;
  readonly #reader;
  #output = "";
  #error: Error | undefined;
  #stopping: Promise<void> | undefined;
  readonly #onError = (error: Error): void => { this.#error = error; };
  readonly #onStderr = (chunk: string): void => { this.#output = (this.#output + chunk).slice(-32_768); };
  readonly #onLine = (line: string): void => { this.#output = (this.#output + line + "\n").slice(-32_768); };

  constructor(command: string, args: string[], cwd: string, env: NodeJS.ProcessEnv, readonly graceful: boolean) {
    this.child = spawn(command, args, { cwd, env, detached: process.platform !== "win32", stdio: ["pipe", "pipe", "pipe"] });
    this.child.stdout.setEncoding("utf8");
    this.child.stderr.setEncoding("utf8");
    this.#reader = createInterface({ input: this.child.stdout });
    this.#reader.on("line", this.#onLine);
    this.child.stderr.on("data", this.#onStderr);
    this.child.on("error", this.#onError);
    this.child.stdout.on("error", this.#onError);
    this.child.stderr.on("error", this.#onError);
    this.child.stdin.on("error", this.#onError);
    if (!graceful) this.child.stdin.end();
  }

  failure(): Error | undefined {
    if (this.#error || this.child.exitCode !== null || this.child.signalCode !== null) {
      return new Error(`${this.child.spawnargs.join(" ")}: ${this.#error?.message ?? `exit ${this.child.exitCode}, signal ${this.child.signalCode}`}\n${this.#output}`);
    }
    return undefined;
  }

  diagnostics(): string { return this.#output; }

  async command(command: "disconnect" | "reconnect" | "reseed"): Promise<void> {
    const failure = this.failure();
    if (failure) throw failure;
    await new Promise<void>((resolve, reject) => {
      const cleanup = (): void => {
        clearTimeout(timer);
        this.#reader.off("line", onLine);
        this.child.off("exit", onExit);
        this.child.off("error", onError);
      };
      const fail = (error: Error): void => { cleanup(); reject(error); };
      const onError = (error: Error): void => fail(error);
      const onExit = (): void => fail(this.failure() ?? new Error("fixture exited"));
      const onLine = (line: string): void => {
        if (line === `BROWSER CONTROL ${command}`) { cleanup(); resolve(); }
      };
      const timer = setTimeout(() => fail(new Error(`fixture control ${command} timed out\n${this.#output}`)), 5000);
      this.#reader.on("line", onLine);
      this.child.once("exit", onExit);
      this.child.once("error", onError);
      this.child.stdin.write(`${command}\n`, error => { if (error) fail(error); });
    });
  }

  stop(): Promise<void> {
    return this.#stopping ??= (async () => {
      this.child.stdin.end(); // EngineCore treats EOF as graceful shutdown.
      if (this.graceful && this.child.exitCode === null && this.child.signalCode === null) {
        await new Promise<void>(resolve => {
          const finish = (): void => { clearTimeout(timer); this.child.off("exit", finish); resolve(); };
          const timer = setTimeout(finish, 1000);
          this.child.once("exit", finish);
        });
      }
      try { await stopChild(this.child, true); }
      finally {
        this.#reader.close();
        this.#reader.off("line", this.#onLine);
        this.child.stderr.off("data", this.#onStderr);
        this.child.off("error", this.#onError);
        this.child.stdout.off("error", this.#onError);
        this.child.stderr.off("error", this.#onError);
        this.child.stdin.off("error", this.#onError);
      }
    })();
  }
}

async function unusedPort(): Promise<number> {
  const server = createServer();
  return await new Promise<number>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (!address || typeof address === "string") { server.close(); reject(new Error("missing fixture port")); return; }
      server.close(error => error ? reject(error) : resolve(address.port));
    });
  });
}

const pause = (ms: number): Promise<void> => new Promise(resolve => setTimeout(resolve, ms));

export interface BrowserRelayEngine {
  readonly deviceId: string;
  readonly ownerId: string;
  readonly organizationId: string;
  readonly label: string;
  readonly projectRoot: string;
  readonly chatId: string;
  readonly spaceId: string;
  readonly child: ChildProcessWithoutNullStreams;
  disconnect(): Promise<void>;
  reconnect(): Promise<void>;
  /** Re-assert fixture seed fields through the native mutation dispatcher. */
  reseed(): Promise<void>;
  stop(): Promise<void>;
  restart(): Promise<void>;
}

export interface BrowserRelaySession {
  readonly ownerId: string;
  readonly organizationId: string;
  readonly csrfToken: string;
  /** Test-runner transport only: never expose this to application JavaScript. */
  readonly cookie: string;
  request(path: string, init?: RequestInit): Promise<Response>;
  relayUrl(deviceId: string): string;
  openSocket(deviceId: string): WebSocket;
  /** Pass to Playwright BrowserContext.addCookies to use native browser cookies. */
  browserCookies(): Array<{ name: string; value: string; url: string; httpOnly: boolean; sameSite: "Strict" }>;
}

export interface BrowserRelayFixture {
  readonly origin: string;
  readonly root: string;
  readonly engines: readonly BrowserRelayEngine[];
  readonly session: BrowserRelaySession;
  readonly edgeChild: ChildProcessWithoutNullStreams;
  addEngine(options: { label: string; ownerId?: string; organizationId?: string; mockDelayMs?: number }): Promise<BrowserRelayEngine>;
  /** Reconfigure loopback dev-login identity, preserving the real DO storage/key. */
  loginAs(ownerId: string, organizationId?: string): Promise<BrowserRelaySession>;
  waitForDevice(session: BrowserRelaySession, engine: BrowserRelayEngine, online?: boolean): Promise<void>;
  stop(): Promise<void>;
}

export interface BrowserRelayOptions {
  ownerId?: string;
  organizationId?: string;
  engineLabels?: string[];
  mockDelayMs?: number;
  /** Register the enabled ClaudeCode-id scripted provider (default true). */
  scriptedClaude?: boolean;
  /** Optional actual built web app for rendered end-to-end consumers. */
  assetsDirectory?: string;
}

/** Actual Wrangler/workerd, BrowserSessionStore, DeviceRoom and EngineCore. */
export async function startBrowserRelayFixture(options: BrowserRelayOptions = {}): Promise<BrowserRelayFixture> {
  const metadata = JSON.parse(await runCommand("cargo", ["metadata", "--no-deps", "--format-version", "1"], 30_000)) as { target_directory: string };
  await runCommand("cargo", ["build", "--locked", "-p", "zeron-engine", "--example", "browser_relay"], 540_000);
  const binary = join(metadata.target_directory, "debug", "examples", `browser_relay${process.platform === "win32" ? ".exe" : ""}`);
  const port = await unusedPort();
  await mkdir(join(repoRoot, ".scratch"), { recursive: true });
  const root = await mkdtemp(join(repoRoot, ".scratch", "browser-relay-"));
  const processes: FixtureProcess[] = [];
  const engines: BrowserRelayEngine[] = [];
  let owner = options.ownerId ?? "relay-owner-a";
  let org = options.organizationId ?? "relay-org";
  const key = randomBytes(32).toString("base64");
  const origin = `http://127.0.0.1:${port}`;
  let edge: FixtureProcess;
  let currentSession: BrowserRelaySession;
  let stopping: Promise<void> | undefined;

  const stop = (): Promise<void> => stopping ??= (async () => {
    const results = await Promise.allSettled(processes.filter(process => process !== edge).map(process => process.stop()));
    if (edge!) results.push(await edge.stop().then(() => ({ status: "fulfilled", value: undefined }) as const, reason => ({ status: "rejected", reason }) as const));
    const failures = results.filter((result): result is PromiseRejectedResult => result.status === "rejected");
    if (failures.length) throw new AggregateError(failures.map(result => result.reason), "relay fixture teardown failed");
    await rm(root, { recursive: true });
  })();

  const startEdge = async (): Promise<void> => {
    // Preserve the strict loopback URL guard inside workerd as well as outside it.
    const args = [join(edgeRoot, "node_modules/wrangler/bin/wrangler.js"), "dev", "--local", "--local-upstream", `127.0.0.1:${port}`, "--upstream-protocol", "http", "--ip", "127.0.0.1", "--port", String(port), "--inspector-port", "0", "--persist-to", join(root, "edge-state")];
    for (const [name, value] of Object.entries({ AUTH_MODE: "dev", BROWSER_DEV_ORIGIN: origin, BROWSER_DEV_OWNER_SUBJECT: owner, BROWSER_DEV_ORGANIZATION_ID: org, BROWSER_SESSION_KEY: key })) {
      args.push("--var", `${name}:${value}`);
    }
    if (options.assetsDirectory) {
      // Asset mode needs the real candidate entry point (native index.ts
      // intentionally does not serve React deep links). Never alter native config.
      args.push("--config", join(edgeRoot, "wrangler.browser-candidate.jsonc"), "--assets", resolve(options.assetsDirectory));
    }
    edge = new FixtureProcess(process.execPath, args, edgeRoot, { ...process.env, CI: "1", NO_COLOR: "1", WRANGLER_SEND_METRICS: "false" }, false);
    processes.push(edge);
    const deadline = Date.now() + 60_000;
    while (Date.now() < deadline) {
      const failure = edge.failure();
      if (failure) throw failure;
      try { if ((await fetch(`${origin}/health`, { signal: AbortSignal.timeout(500) })).ok) return; } catch { /* booting */ }
      await pause(100);
    }
    throw new Error(`edge startup timed out after 60000ms\n${edge.diagnostics()}`);
  };

  const loginAs = async (ownerId: string, organizationId = org): Promise<BrowserRelaySession> => {
    if (ownerId !== owner || organizationId !== org) {
      await edge.stop();
      owner = ownerId;
      org = organizationId;
      await startEdge();
    }
    const response = await fetch(`${origin}/api/browser/dev-login`, { method: "POST", headers: { origin }, signal: AbortSignal.timeout(10_000) });
    if (!response.ok) throw new Error(`dev cookie login HTTP ${response.status}: ${await response.text()}\n${edge.diagnostics()}`);
    const cookieHeader = response.headers.get("set-cookie");
    const body = await response.json() as { ownerId: string; csrfToken: string };
    if (!cookieHeader?.includes("HttpOnly") || body.ownerId !== ownerId || !body.csrfToken) throw new Error("dev login did not mint a real owner-scoped HttpOnly session");
    const cookie = cookieHeader.split(";")[0]!;
    const session: BrowserRelaySession = {
      ownerId, organizationId, csrfToken: body.csrfToken, cookie,
      request: (path, init = {}) => {
        const url = new URL(path, origin);
        if (url.origin !== origin) throw new Error("fixture cookies cannot leave loopback origin");
        const headers = new Headers(init.headers);
        headers.set("cookie", cookie);
        headers.set("origin", origin);
        return fetch(url, { ...init, headers, signal: init.signal ?? AbortSignal.timeout(10_000) });
      },
      relayUrl: deviceId => `${origin.replace("http:", "ws:")}/api/browser/device/${encodeURIComponent(deviceId)}/ws`,
      openSocket: deviceId => new WebSocket(session.relayUrl(deviceId), { headers: { Cookie: cookie, Origin: origin }, handshakeTimeout: 10_000 }),
      browserCookies: () => {
        const at = cookie.indexOf("=");
        return [{ name: cookie.slice(0, at), value: cookie.slice(at + 1), url: origin, httpOnly: true, sameSite: "Strict" }];
      },
    };
    currentSession = session;
    return session;
  };

  const waitForDevice = async (session: BrowserRelaySession, engine: BrowserRelayEngine, online = true): Promise<void> => {
    const deadline = Date.now() + 30_000;
    let last = "no response";
    while (Date.now() < deadline) {
      const failure = edge.failure();
      if (failure) throw failure;
      const response = await session.request("/api/browser/devices");
      last = await response.text();
      if (!response.ok) throw new Error(`device discovery HTTP ${response.status}: ${last}`);
      const body = JSON.parse(last) as { devices: Array<{ id: string; online: boolean }> };
      if (body.devices.some(device => device.id === engine.deviceId && device.online === online)) return;
      await pause(100);
    }
    throw new Error(`device ${engine.deviceId} online=${online} timed out: ${last}\n${edge.diagnostics()}`);
  };

  const addEngine: BrowserRelayFixture["addEngine"] = async engineOptions => {
    const label = engineOptions.label;
    if (!/^[A-Za-z0-9_-]{1,128}$/.test(label) || engines.some(engine => engine.label === label)) throw new Error(`invalid or duplicate fixture engine label: ${label}`);
    const engineOwner = engineOptions.ownerId ?? owner;
    const engineOrg = engineOptions.organizationId ?? org;
    const dataDir = join(root, label);
    let process: FixtureProcess;
    let info: Omit<BrowserRelayEngine, "child" | "stop" | "restart" | "disconnect" | "reconnect">;
    const launch = async (): Promise<void> => {
      process = new FixtureProcess(binary, [origin, dataDir, engineOwner, engineOrg, label], repoRoot, {
        ...globalThis.process.env,
        ZERON_MOCK_DELAY_MS: String(engineOptions.mockDelayMs ?? options.mockDelayMs ?? 80),
        ZERON_MOCK_QUESTION: "0",
        ZERON_BROWSER_RELAY_SCRIPTED_CLAUDE: options.scriptedClaude === false ? "0" : "1",
      }, true);
      processes.push(process);
      try {
        info = JSON.parse(await waitForReady(process.child, "BROWSER RELAY ", 15_000));
      } catch (error) { await process.stop(); throw error; }
    };
    await launch();
    const engine: BrowserRelayEngine = {
      ...info!,
      get child() { return process.child; },
      disconnect: () => process.command("disconnect"),
      reconnect: () => process.command("reconnect"),
      reseed: () => process.command("reseed"),
      stop: () => process.stop(),
      restart: async () => {
        const deviceId = info.deviceId;
        await process.stop();
        await launch();
        if (info.deviceId !== deviceId) throw new Error("fixture restart changed device identity");
      },
    };
    engines.push(engine);
    return engine;
  };

  try {
    await startEdge();
    await loginAs(owner, org);
    for (const label of options.engineLabels ?? ["engine-a", "engine-b"]) await addEngine({ label });
    for (const engine of engines) await waitForDevice(currentSession!, engine);
    // The initial seeds can race the two peers' first workspace merge. Reapply
    // their titles only after both actual relay links are registered.
    for (const engine of engines) await engine.reseed();
    return {
      origin, root, engines,
      get session() { return currentSession; },
      get edgeChild() { return edge.child; },
      addEngine, loginAs, waitForDevice, stop,
    };
  } catch (error) {
    try { await stop(); } catch (cleanup) { throw new AggregateError([error, cleanup], "relay startup and teardown failed"); }
    throw error;
  }
}
