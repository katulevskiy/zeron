import type { EngineCacheStore } from "@zeron/engine-client";
import type { TranscriptCache } from "./transcript-store";
import { isCurrentPrivateSession, privateSessionGeneration } from "./private-session-generation";

// Timestamps reveal visited chat IDs too; they share the transcript's lifetime.
const stamps = new Map<string, Map<string, number>>();

export function resetPrivateTranscriptCache(): void {
  stamps.clear();
}

/** Capture the session at handle creation so a delayed old transcript cannot
 * write into the next owner's engine cache, even when device/chat IDs match. */
export function privateTranscriptCacheFor(cache: EngineCacheStore, engineKey: string, chatId: string): TranscriptCache {
  const generation = privateSessionGeneration();
  return {
    load: async () => {
      if (!isCurrentPrivateSession(generation)) return null;
      const entries = await cache.loadTranscript(engineKey, chatId);
      if (!isCurrentPrivateSession(generation) || entries === null) return null;
      return { entries, savedAtMs: stamps.get(engineKey)?.get(chatId) ?? 0 };
    },
    save: async (entries) => {
      if (!isCurrentPrivateSession(generation)) return;
      await cache.saveTranscript(engineKey, chatId, entries);
      if (!isCurrentPrivateSession(generation)) return;
      let chats = stamps.get(engineKey);
      if (chats === undefined) { chats = new Map(); stamps.set(engineKey, chats); }
      chats.set(chatId, Date.now());
    },
  };
}
