import type { CachedRows, EngineCacheStore } from "@zeron/engine-client";
import type { SessionMessageEntry } from "@zeron/proto";
import { chatDrafts, composerDefaults } from "../lib/composer-draft";
import { resetPrivateChatSeen } from "../lib/chat-actions";
import { projectActionsStore } from "../lib/project-actions";
import { projectIconsStore } from "../lib/project-icons";
import { resetAttachmentCache } from "./attachment-cache";
import { fileDocuments } from "./file-documents";
import { resetPrivateHistoryStores } from "./history-store";
import { rightPaneStore } from "./right-pane";
import { changesSurfaceStore } from "./changes-surface";
import { reviewCommentStore } from "./review-comments";
import { echoStore, pendingQueuedTurns, savedViewportCache } from "./transcript-store";
import { transcriptFoldCache } from "./transcript-fold-state";
import { sidebarStore } from "./sidebar";
import { navHistory } from "./nav-history";
import { chromeStore } from "./chrome";
import { sidebarNotice } from "./notice";
import { addSpaceStore } from "./add-space";
import { commandPaletteStore } from "./command-palette";
import { advancePrivateSessionGeneration, isCurrentPrivateSession, privateSessionGeneration } from "./private-session-generation";
import { resetPrivateTranscriptCache } from "./private-transcript-cache";

let scope: string | null = null;
let pendingBegin: { scope: string; promise: Promise<void> } | null = null;
const rows = new Map<string, CachedRows>();
const transcripts = new Map<string, Map<string, readonly SessionMessageEntry[]>>();

/** Authenticated-window memory only. Never reads or writes an ownerless disk cache. */
export const browserEngineCache: EngineCacheStore = {
  async loadRows(key) {
    const generation = privateSessionGeneration();
    const value = scope === null ? null : rows.get(key) ?? null;
    await Promise.resolve();
    return scope !== null && isCurrentPrivateSession(generation) && value !== null ? structuredClone(value) : null;
  },
  async saveRows(key, value) {
    if (scope !== null) rows.set(key, structuredClone(value));
  },
  async loadTranscript(key, chatId) {
    const generation = privateSessionGeneration();
    const value = scope === null ? null : transcripts.get(key)?.get(chatId) ?? null;
    await Promise.resolve();
    return scope !== null && isCurrentPrivateSession(generation) && value !== null ? structuredClone(value) : null;
  },
  async saveTranscript(key, chatId, value) {
    if (scope === null) return;
    let chats = transcripts.get(key);
    if (chats === undefined) { chats = new Map(); transcripts.set(key, chats); }
    chats.set(chatId, structuredClone(value));
  },
  async forgetEngine(key) {
    rows.delete(key);
    transcripts.delete(key);
  },
};

function resetPrivateState(): void {
  advancePrivateSessionGeneration();
  rows.clear();
  transcripts.clear();
  resetPrivateTranscriptCache();
  chatDrafts.reset();
  composerDefaults.resetPrivateTarget();
  echoStore.reset();
  savedViewportCache.clear();
  pendingQueuedTurns.clear();
  transcriptFoldCache.clear();
  resetAttachmentCache();
  reviewCommentStore.resetPrivateState();
  rightPaneStore.resetPrivateState();
  fileDocuments.resetPrivateState();
  changesSurfaceStore.resetPrivateState();
  resetPrivateHistoryStores();
  projectActionsStore.resetPrivateState();
  projectIconsStore.dropAll();
  resetPrivateChatSeen();
  sidebarStore.resetPrivateState();
  navHistory.resetPrivateState();
  chromeStore.clear();
  sidebarNotice.clear();
  addSpaceStore.resetPrivateState();
  commandPaletteStore.resetPrivateState();
}

/** Parent must drain registry cache writes BEFORE switching scope and await this
 * promise BEFORE mounting authenticated state. A failed purge leaves cache disabled.
 * Same-scope polling preserves both active state and pending activation. */
export function beginPrivateSession(nextScope: string): Promise<void> {
  if (nextScope.trim().length === 0) return Promise.reject(new Error("Private session scope is required"));
  if (scope === nextScope) return Promise.resolve();
  if (pendingBegin?.scope === nextScope) return pendingBegin.promise;
  scope = null;
  resetPrivateState();
  const generation = privateSessionGeneration();
  const promise = purgeLegacyPrivateCaches().then(() => {
    if (!isCurrentPrivateSession(generation)) {
      throw new Error("Private session activation was superseded");
    }
    scope = nextScope;
  }).finally(() => {
    if (pendingBegin?.promise === promise) pendingBegin = null;
  });
  pendingBegin = { scope: nextScope, promise };
  return promise;
}

/** Invalidates memory synchronously; resolves only after targeted legacy disk retirement.
 * Registry shutdown/flush and authenticated-root unmount are owned by the caller. */
export async function endPrivateSession(): Promise<void> {
  scope = null;
  pendingBegin = null;
  resetPrivateState();
  await purgeLegacyPrivateCaches();
}

let purgeInFlight: Promise<void> | null = null;
function purgeLegacyPrivateCaches(): Promise<void> {
  // These are the old direct-access credentials and ownerless transcript stamps,
  // not appearance, themes/background artwork, or unrelated origin storage.
  try {
    const storage = globalThis.localStorage;
    storage?.removeItem("zeron.fleet.v1");
    storage?.removeItem("zeron.transcriptSeedStamps.v1");
    storage?.removeItem("zeron.sidebar.v1");
  } catch {
    return Promise.reject(new Error("Legacy private browser storage could not be removed"));
  }
  if (purgeInFlight !== null) return purgeInFlight;
  const factory = globalThis.indexedDB;
  if (factory === undefined) return Promise.resolve();
  purgeInFlight = new Promise<void>((resolve, reject) => {
    const request = factory.deleteDatabase("zeron-engine-cache");
    request.onsuccess = () => resolve();
    request.onerror = () => reject(new Error("Legacy private engine cache could not be removed"));
    request.onblocked = () => reject(new Error("Legacy private engine cache is open in another tab; close it and retry"));
  }).finally(() => { purgeInFlight = null; });
  return purgeInFlight;
}

function reportPurgeFailure(error: unknown): void {
  console.warn("Private cache retirement needs retry", error);
}

// Importing the browser boundary starts signed out: retire old private bytes
// before any authenticated viewport is mounted. No legacy cache is ever loaded.
void endPrivateSession().catch(reportPurgeFailure);
