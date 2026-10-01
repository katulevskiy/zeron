import { EngineClient, EngineWatchCache } from "@zeron/engine-client";
import type { OwnedEngine } from "../lib/owned-engine";
import { PickerCatalog } from "./picker-catalog";
import { TranscriptPool } from "./transcript-pool";

/** App resources for one cookie-discovered device. The registry owns transport
 * and watches; this wrapper owns only its catalog and warm transcripts. */
export interface EngineSession {
  readonly engine: OwnedEngine;
  readonly client: EngineClient;
  readonly cache: EngineWatchCache;
  readonly catalog: PickerCatalog;
  readonly transcripts: TranscriptPool;
}

export function engineSessionKey(engine: OwnedEngine): string {
  return `${engine.key}\n${engine.endpoint}`;
}

/** Compose already-supervised resources; never dial or manufacture credentials. */
export function createEngineSession(
  engine: OwnedEngine,
  client: EngineClient,
  cache: EngineWatchCache,
): EngineSession {
  const catalog = new PickerCatalog(client);
  return { engine, client, cache, catalog, transcripts: new TranscriptPool(client) };
}

export function disposeEngineSession(session: EngineSession): void {
  session.catalog.dispose();
  session.transcripts.dispose();
}

export interface SessionResources {
  readonly client: EngineClient;
  readonly cache: EngineWatchCache;
}

export interface SessionReconciliation {
  readonly sessions: ReadonlyMap<string, EngineSession>;
  /** Displaced owned resources, not metadata-refreshed wrappers. */
  readonly displaced: readonly EngineSession[];
}

/** Metadata refresh retains live resources. Registry replacement (including
 * Retry) replaces them even when discovery metadata is unchanged. Disposal
 * stays with the caller; transport/cache ownership stays with the registry. */
export function reconcileEngineSessions(
  previous: ReadonlyMap<string, EngineSession>,
  engines: readonly OwnedEngine[],
  currentResources: (key: string) => SessionResources | null,
): SessionReconciliation {
  const next = new Map<string, EngineSession>();
  for (const engine of engines) {
    const resources = currentResources(engine.key);
    if (resources === null) continue;
    const existing = previous.get(engine.key);
    if (
      existing !== undefined &&
      existing.engine.endpoint === engine.endpoint &&
      existing.client === resources.client &&
      existing.cache === resources.cache
    ) {
      next.set(engine.key, existing.engine === engine ? existing : { ...existing, engine });
      continue;
    }
    next.set(engine.key, createEngineSession(engine, resources.client, resources.cache));
  }
  const retained = new Set<PickerCatalog>();
  for (const session of next.values()) retained.add(session.catalog);
  const displaced = [...previous.values()].filter((session) => !retained.has(session.catalog));
  let unchanged = next.size === previous.size;
  if (unchanged) {
    for (const [key, session] of next) {
      if (previous.get(key) !== session) {
        unchanged = false;
        break;
      }
    }
  }
  return { sessions: unchanged ? previous : next, displaced };
}
