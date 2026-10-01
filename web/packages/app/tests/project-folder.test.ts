import { describe, expect, it } from "vitest";
import { createProjectRepository, inspectProjectFolder, type ProjectFolderClient } from "../src/lib/project-folder";

describe("selected-project supported RPC seam", () => {
  it("validates exact device paths with ListFolders only", async () => {
    const calls: Array<[string, unknown]> = [];
    const client: ProjectFolderClient = { async call<T>(method: string, params: Record<string, unknown>) { calls.push([method, params]); return { path: "C:\\Projects", entries: [], truncated: false } as T; } };
    await inspectProjectFolder(client, "C:\\Projects");
    expect(calls).toEqual([["ListFolders", { path: "C:\\Projects" }]]);
  });
  it("creates only an explicitly named upstream managed repository", async () => {
    const calls: Array<[string, unknown]> = [];
    const client: ProjectFolderClient = { async call<T>(method: string, params: Record<string, unknown>) { calls.push([method, params]); return { path: "/engine/repos/project" } as T; } };
    await createProjectRepository(client, " project ");
    expect(calls).toEqual([["CreateRepo", { name: "project" }]]);
  });
  it.each(["", "../outside", "/missing/folder", "C:\\Projects", "...", "---", "with spaces"]) ("rejects invalid repository name %j before any filesystem RPC", async name => {
    let called = false;
    const client: ProjectFolderClient = { async call<T>() { called = true; return {} as T; } };
    await expect(createProjectRepository(client, name)).rejects.toThrow(/repository name/);
    expect(called).toBe(false);
  });
});
