import { Outlet } from "@tanstack/react-router";
import { EngineSessionProvider, useEngineRetry, useEngineSession } from "../state/session-provider";
import { engineRegistry, retryBrowserSession, signIn, signOut, useFleet, useFleetRegistry } from "../state/fleet";
import { BrowserSessionGate } from "../components/browser-session-gate";
import { useEngineStatus } from "../state/hooks";
import { GateCard } from "../components/gate-card";
import { useConnectionState } from "../components/connection-state";

/**
 * The root route: the engine session provider, then the gate/page split —
 * the desktop's `GatePhase` (`shell.rs:238-240`), generalized to the fleet:
 *
 * - **Ready** — the page, wrapped in the one keyed `fade_in("phase-app")`
 *   entrance: 500ms `EASE_OUT_EXPO`, opacity 0→1 with a 4px rise, replayed
 *   whenever the app recovers from a gate. ANY engine connecting (or the
 *   offline cache seeding rows) ends the Loading phase — the desktop's
 *   `Ready` persists through registry-level reconnects the same way, and
 *   remote engines' outages are per-entry badges, never a gate.
 * - **Failed** — EVERY engine parked (revoked credential / identity
 *   changed): nothing can connect, so the gate card owns the screen. A
 *   single parked engine while others are live is NOT a gate — its rows
 *   keep rendering from cache and the shell banner carries the state.
 * - **Loading** — the first dial is in flight and nothing has connected or
 *   seeded yet: a plain empty root, no overlay (the boot splash is a
 *   deliberate open question and is NOT built here).
 * Browser authentication is an outer gate; no manual pairing fallback exists.
 */
export function RootLayout() {
  const fleet = useFleet();
  if (!fleet.session.authenticated) {
    return <BrowserSessionGate
      status={fleet.status}
      error={fleet.configurationError}
      onSignIn={() => { void signIn(); }}
      onRetry={() => { void retryBrowserSession().catch(() => {}); }}
      onRetrySignOut={() => { void signOut().catch(() => {}); }}
    />;
  }
  return (
    <EngineSessionProvider>
      <GateAndPage />
    </EngineSessionProvider>
  );
}

function GateAndPage() {
  const session = useEngineSession();
  const status = useEngineStatus(session);
  const state = useConnectionState(status);
  const fleet = useFleet();
  const registry = useFleetRegistry();
  const retry = useEngineRetry();

  const engines = registry.engines;
  const paired = fleet.engines.length > 0;
  // Registry `off` covers both a suspended owned host and fatal parking.
  // Only the latter can gate the page; offline sessions remain read-only.
  const offline = engines.some((engine) => engineRegistry.clientFor(engine.key)?.state === "offline");
  const allFailed = paired && engines.length > 0 && !offline && engines.every((engine) => engine.state === "off");
  const anythingLive =
    offline || engines.some((engine) => engine.state === "connected" || engine.chats.loaded || engine.spaces.loaded);

  const phase = paired && engines.length > 0 ? (allFailed ? "failed" : anythingLive ? "ready" : "loading") : "ready";

  if (phase === "loading") {
    // GatePhase::Loading — the desktop renders ONLY the root (plus a splash
    // this port deliberately defers): nothing to see, nothing to click.
    return <div className="gate-loading" />;
  }
  if (phase === "failed") {
    const error = engines.find((engine) => engine.state === "off" && engine.lastError !== null)?.lastError
      ?? engines[0]?.lastError
      ?? state.detail
      ?? state.label;
    return (
      <GateCard error={error ?? "Engine connection failed"} onRetry={retry}>
        <button type="button" className="gate-session-action" onClick={() => { void signOut().catch(() => {}); }}>
          Sign out
        </button>
      </GateCard>
    );
  }
  return (
    <div className="page-fade" key={phase}>
      <Outlet />
    </div>
  );
}
