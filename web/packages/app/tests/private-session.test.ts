import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CachedRows } from "@zeron/engine-client";
import type { SessionMessageEntry } from "@zeron/proto";
import { beginPrivateSession, browserEngineCache, endPrivateSession } from "../src/state/private-session";
import { privateTranscriptCacheFor } from "../src/state/private-transcript-cache";
import { isCurrentPrivateSession, privateSessionGeneration } from "../src/state/private-session-generation";
import { chatDrafts, composerDefaults } from "../src/lib/composer-draft";
import { echoStore, pendingQueuedTurns, savedViewportCache } from "../src/state/transcript-store";
import { transcriptFoldCache } from "../src/state/transcript-fold-state";
import { seedAttachment, getAttachmentSnapshot, loadAttachment, subscribeAttachment, beginUploadProgress, uploadProgressPercent } from "../src/state/attachment-cache";
import { uiSettings } from "../src/state/ui-settings";
import { sidebarStore } from "../src/state/sidebar";
import { navHistory } from "../src/state/nav-history";
import { rightPaneStore } from "../src/state/right-pane";
import { reviewCommentStore } from "../src/state/review-comments";
import { commandPaletteStore } from "../src/state/command-palette";
import { newFileComment } from "../src/lib/review-comments";
import { FileDocument } from "../src/lib/file-document";
import { FileTreeModel } from "../src/lib/file-tree";
import { WorkspaceFilesClient } from "../src/lib/files-client";
import { fileDocuments } from "../src/state/file-documents";
const cachedRows: CachedRows = { chats: [], spaces: [], devices: [], sessions: [] };
const entries: SessionMessageEntry[] = [{ id: "private-message", role: "user", parts: [{ kind: "text", id: "private-part", text: "owner A private prompt" }], createdAt: 1, deviceId: "device" }];

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

let disk: Map<string, string>;
let deleteDatabase: ReturnType<typeof vi.fn>;
beforeEach(async () => {
  disk = new Map();
  vi.stubGlobal("localStorage", { getItem: (key: string) => disk.get(key) ?? null, setItem: (key: string, value: string) => disk.set(key, value), removeItem: (key: string) => disk.delete(key) });
  deleteDatabase = vi.fn((_name: string) => {
    const request = { onsuccess: null as (() => void) | null, onerror: null as (() => void) | null, onblocked: null as (() => void) | null };
    queueMicrotask(() => request.onsuccess?.());
    return request;
  });
  vi.stubGlobal("indexedDB", { deleteDatabase });
  await endPrivateSession();
});
afterEach(async () => { await endPrivateSession(); vi.unstubAllGlobals(); });

describe("browser private-session lifecycle", () => {
  it.each([false, true])("rejects blocked authentication purge (existing session: %s) and permits retry", async (existingSession) => {
    if (existingSession) {
      await beginPrivateSession("session A");
      await browserEngineCache.saveTranscript("device", "chat", entries);
      chatDrafts.set("private-chat", "secret draft");
    }
    const request = { onblocked: null as (() => void) | null };
    deleteDatabase.mockImplementationOnce(() => request);
    const starting = Promise.resolve(beginPrivateSession("session B"));
    const outcome = starting.then(() => "ready", (error: Error) => error.message);
    request.onblocked?.();
    expect(await outcome).toContain("open in another tab");
    expect(chatDrafts.get("private-chat")).toBe("");
    await browserEngineCache.saveTranscript("device", "chat", entries);
    expect(await browserEngineCache.loadTranscript("device", "chat")).toBeNull();
    await beginPrivateSession("session B");
    await browserEngineCache.saveTranscript("device", "chat", entries);
    expect(await browserEngineCache.loadTranscript("device", "chat")).toEqual(entries);
  });

  it("keeps cache disabled until purge completes and shares pending same-scope activation", async () => {
    const request = { onsuccess: null as (() => void) | null };
    deleteDatabase.mockImplementationOnce(() => request);
    let ready = false;
    const starting = Promise.resolve(beginPrivateSession("session A")).then(() => { ready = true; });
    const generation = privateSessionGeneration();
    const polling = Promise.resolve(beginPrivateSession("session A"));
    const generationAfterPolling = privateSessionGeneration();
    await browserEngineCache.saveRows("device", cachedRows);
    const beforePurge = await browserEngineCache.loadRows("device");
    const readyBeforePurge = ready;
    request.onsuccess?.();
    await Promise.all([starting, polling]);
    expect(readyBeforePurge).toBe(false);
    expect(beforePurge).toBeNull();
    expect(generationAfterPolling).toBe(generation);
    await browserEngineCache.saveRows("device", cachedRows);
    await beginPrivateSession("session A");
    expect(await browserEngineCache.loadRows("device")).toEqual(cachedRows);
  });

  it("does not reactivate a scope when an old begin finishes after sign-out", async () => {
    const request = { onsuccess: null as (() => void) | null };
    deleteDatabase.mockImplementationOnce(() => request);
    const starting = Promise.resolve(beginPrivateSession("session A"));
    const outcome = starting.then(() => "ready", (error: Error) => error.message);
    const ending = endPrivateSession();
    request.onsuccess?.();
    await ending;
    expect(await outcome).toContain("superseded");
    await browserEngineCache.saveRows("device", cachedRows);
    expect(await browserEngineCache.loadRows("device")).toBeNull();
  });

  it("rejects superseded activation when a newer account shares the pending purge", async () => {
    const request = { onsuccess: null as (() => void) | null };
    deleteDatabase.mockImplementationOnce(() => request);
    const first = Promise.resolve(beginPrivateSession("session A"));
    const outcome = first.then(() => "ready", (error: Error) => error.message);
    const second = Promise.resolve(beginPrivateSession("session B"));
    request.onsuccess?.();
    await second;
    expect(await outcome).toContain("superseded");
    await browserEngineCache.saveRows("device", cachedRows);
    await beginPrivateSession("session B");
    expect(await browserEngineCache.loadRows("device")).toEqual(cachedRows);
  });

  it("cannot bypass a blocked import-time retirement by starting authentication", async () => {
    const request = { onblocked: null as (() => void) | null };
    deleteDatabase.mockImplementationOnce(() => request);
    const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.resetModules();
    try {
      const fresh = await import("../src/state/private-session");
      const starting = Promise.resolve(fresh.beginPrivateSession("session A"));
      const outcome = starting.then(() => "ready", (error: Error) => error.message);
      request.onblocked?.();
      expect(await outcome).toContain("open in another tab");
      await fresh.browserEngineCache.saveRows("device", cachedRows);
      expect(await fresh.browserEngineCache.loadRows("device")).toBeNull();
      await fresh.endPrivateSession();
    } finally {
      warning.mockRestore();
    }
  });
  it("does not load or retain cache writes while signed out", async () => {
    await browserEngineCache.saveRows("device", cachedRows);
    await browserEngineCache.saveTranscript("device", "chat", entries);
    await beginPrivateSession("session A");
    expect(await browserEngineCache.loadRows("device")).toBeNull();
    expect(await browserEngineCache.loadTranscript("device", "chat")).toBeNull();
  });

  it("retains during the same session, isolates identical device/chat IDs after A to B, and forgets only one engine", async () => {
    await beginPrivateSession("session A");
    await browserEngineCache.saveRows("device", cachedRows);
    await browserEngineCache.saveTranscript("device", "chat", entries);
    await browserEngineCache.saveRows("other", cachedRows);
    await beginPrivateSession("session A");
    expect(await browserEngineCache.loadTranscript("device", "chat")).toEqual(entries);
    await browserEngineCache.forgetEngine("other");
    expect(await browserEngineCache.loadRows("other")).toBeNull();
    expect(await browserEngineCache.loadRows("device")).toEqual(cachedRows);
    const pendingRows = browserEngineCache.loadRows("device");
    const pendingTranscript = browserEngineCache.loadTranscript("device", "chat");
    await beginPrivateSession("session B");
    expect(await pendingRows).toBeNull();
    expect(await pendingTranscript).toBeNull();
    expect(await browserEngineCache.loadRows("device")).toBeNull();
    expect(await browserEngineCache.loadTranscript("device", "chat")).toBeNull();
    await browserEngineCache.saveTranscript("device", "chat", []);
    await endPrivateSession();
    expect(await browserEngineCache.loadTranscript("device", "chat")).toBeNull();
  });

  it("rejects old transcript handles after a session change without writing IDs or timestamps to storage", async () => {
    await beginPrivateSession("session A");
    const oldHandle = privateTranscriptCacheFor(browserEngineCache, "device", "chat");
    await oldHandle.save(entries);
    expect((await oldHandle.load())?.entries).toEqual(entries);
    await beginPrivateSession("session B");
    await oldHandle.save(entries);
    expect(await oldHandle.load()).toBeNull();
    expect(await browserEngineCache.loadTranscript("device", "chat")).toBeNull();
    expect([...disk.values()].join(" ")).not.toContain("private-message");
    expect(disk.has("zeron.transcriptSeedStamps.v1")).toBe(false);
  });

  it("clears all private singleton snapshots synchronously and preserves nonprivate customization", async () => {
    await beginPrivateSession("session A");
    const oldGeneration = privateSessionGeneration();
    chatDrafts.set("private-chat", "secret draft");
    composerDefaults.update({ device: "device A", project: "private-project", harness: "codex" });
    echoStore.pushEcho({ messageId: "echo", chatId: "private-chat", text: "secret echo", startedAtMs: 1, attachmentPaths: [] });
    savedViewportCache.save("private-chat", { kind: "followTail" });
    pendingQueuedTurns.register("private-chat", "message");
    transcriptFoldCache.capture("device", "private-chat", { groups: new Map([["private-group", true]]), details: new Map() });
    seedAttachment("device", "private-path", { name: "secret.png", mime: "image/png", bytes: new Uint8Array([1]) });
    beginUploadProgress(50);
    uiSettings.update({ sidebarWidth: 300, appearance: "dark", lastSpaceId: "private-project", spaceFilter: "private-project", sidebarPinnedSessionIdsByProfile: { a: ["private-chat"] } });
    navHistory.visit({ kind: "chat", chatId: "private-chat" });
    rightPaneStore.openFilesPanel("private-chat");
    commandPaletteStore.open();
    reviewCommentStore.addComment("private-chat", newFileComment("private-path", 1, "secret review"));
    expect(reviewCommentStore.snapshotFor("private-chat").comments).toHaveLength(1);
    const changed = vi.fn();
    const unsubscribe = subscribeAttachment("device", "private-path", changed);
    const pending = endPrivateSession();
    expect(isCurrentPrivateSession(oldGeneration)).toBe(false);
    expect(chatDrafts.get("private-chat")).toBe("");
    expect(composerDefaults.getSnapshot()).toMatchObject({ device: null, project: null, harness: "codex" });
    expect(echoStore.forChat("private-chat")).toEqual([]);
    expect(savedViewportCache.get("private-chat")).toBeUndefined();
    expect(pendingQueuedTurns.size).toBe(0);
    expect(transcriptFoldCache.restore("device", "private-chat")).toBeNull();
    expect(getAttachmentSnapshot("device", "private-path").image).toBeNull();
    expect(uploadProgressPercent()).toBeNull();
    expect(changed).toHaveBeenCalledOnce();
    expect(uiSettings.getSnapshot()).toMatchObject({ sidebarWidth: 300, appearance: "dark", lastSpaceId: null, spaceFilter: null });
    expect(sidebarStore.getSnapshot().pinnedByProfile).toEqual({});
    expect(navHistory.current()).toEqual({ kind: "chat", chatId: "" });
    expect(rightPaneStore.stateFor("private-chat").filesOpen).toBe(false);
    expect(reviewCommentStore.snapshotFor("private-chat").comments).toEqual([]);
    expect(commandPaletteStore.getSnapshot()).toMatchObject({ status: "closed", query: "" });
    unsubscribe();
    await pending;
  });

  it("drops attachment bytes that finish after logout or an account switch", async () => {
    await beginPrivateSession("session A");
    const read = deferred<unknown>();
    const loading = loadAttachment({ call: () => read.promise }, "device", "private-path");
    await beginPrivateSession("session B");
    read.resolve({ name: "secret.png", mimeType: "image/png", data: "AQ==", nextOffset: 1, done: true });
    await loading;
    expect(getAttachmentSnapshot("device", "private-path").image).toBeNull();
  });

  it("disposes file surfaces and ignores private file reads completing in a new session", async () => {
    await beginPrivateSession("session A");
    const read = deferred<unknown>();
    const client = new WorkspaceFilesClient({ call: <T>() => read.promise as Promise<T> }, { spaceId: "private-space" });
    const document = new FileDocument(client, "private-path");
    const model = new FileTreeModel({ client });
    const detach = fileDocuments.attach("private-surface", { document, model, path: "private-path" });
    document.load();
    const changed = vi.fn();
    document.subscribe(changed);
    await beginPrivateSession("session B");
    expect(fileDocuments.entryFor("private-surface")).toBeNull();
    read.resolve({ path: "private-path", text: "private file content", contentHash: "hash", checkoutId: "checkout", readOnly: false });
    for (let i = 0; i < 6; i += 1) await Promise.resolve();
    expect(changed).not.toHaveBeenCalled();
    expect(document.getSnapshot().text).toBe("");
    detach();
  });
  it("removes only documented legacy private keys/database and keeps logout-pending", async () => {
    for (const key of ["zeron.fleet.v1", "zeron.transcriptSeedStamps.v1", "zeron.sidebar.v1"]) disk.set(key, "legacy private data");
    disk.set("zeron.browser.logout-pending", "true");
    disk.set("zeron.appearance.v1", "custom appearance");
    disk.set("another-app", "unrelated data");
    await endPrivateSession();
    expect([...disk.keys()].sort()).toEqual(["another-app", "zeron.appearance.v1", "zeron.browser.logout-pending"]);
    expect(deleteDatabase.mock.calls.every(([name]) => name === "zeron-engine-cache")).toBe(true);
  });

  it("clears memory before reporting a blocked legacy database deletion and allows retry", async () => {
    await beginPrivateSession("session A");
    await browserEngineCache.saveTranscript("device", "chat", entries);
    // Let the previous successful retirement settle before the blocked attempt.
    await Promise.resolve(); await Promise.resolve();
    deleteDatabase.mockImplementationOnce(() => {
      const request = { onblocked: null as (() => void) | null };
      queueMicrotask(() => request.onblocked?.());
      return request;
    });
    const ending = endPrivateSession();
    expect(await browserEngineCache.loadTranscript("device", "chat")).toBeNull();
    await expect(ending).rejects.toThrow("open in another tab");
    await endPrivateSession();
  });
});
