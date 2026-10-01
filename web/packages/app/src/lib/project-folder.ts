import { UPSTREAM_METHODS, type FolderListing, type Repo } from "@zeron/proto";

export interface ProjectFolderClient {
  call<T = unknown>(method: string, params: Record<string, unknown>): Promise<T>;
}

/** Validation happens on the owning engine; never create a typed path. */
export function inspectProjectFolder(client: ProjectFolderClient, path: string): Promise<FolderListing> {
  return client.call<FolderListing>(UPSTREAM_METHODS.LIST_FOLDERS, { path });
}

/** Upstream creates repositories in its managed repos directory, not at an arbitrary path. */
export function createProjectRepository(client: ProjectFolderClient, name: string): Promise<Repo> {
  const clean = name.trim();
  if (!/^[a-zA-Z0-9._-]+$/.test(clean) || /^[.-]+$/.test(clean)) {
    return Promise.reject(new Error("Use a repository name containing letters, numbers, dots, underscores or hyphens (not a path)."));
  }
  return client.call<Repo>(UPSTREAM_METHODS.CREATE_REPO, { name: clean });
}
