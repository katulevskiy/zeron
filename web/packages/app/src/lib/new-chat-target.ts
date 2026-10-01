import { parseScopedId } from "@zeron/engine-client";

/** The target fields persisted by the new-chat composer. */
export interface NewChatDefaults {
  readonly device: string | null;
  readonly project: string | null;
  readonly noProject: boolean;
}

/** The sidebar's navigation state, used only when the canvas has no opt-out. */
export interface NewChatSidebarTarget {
  readonly spaceFilter: string | null;
  readonly lastSpaceId: string | null;
}

/**
 * One resolved new-chat target for every canvas consumer. The project wins when
 * present; an explicit projectless choice excludes sidebar history entirely.
 * Scoped identities name the owning engine, so routing reaches that owner before
 * the wire boundary validates and decodes the request identities.
 */
export interface NewChatTarget {
  readonly projectId: string | null;
  readonly deviceId: string | null;
  readonly noProject: boolean;
  readonly engineKey: string | null;
}

export function resolveNewChatTarget(
  defaults: NewChatDefaults,
  sidebar: NewChatSidebarTarget,
  activeEngine: string | null,
): NewChatTarget {
  const projectId = defaults.noProject ? null : (defaults.project ?? sidebar.spaceFilter ?? sidebar.lastSpaceId);
  const engineKey = ownerOf(projectId, activeEngine) ?? ownerOf(defaults.device, activeEngine) ?? activeEngine;
  return { projectId, deviceId: defaults.device, noProject: defaults.noProject, engineKey };
}

function ownerOf(id: string | null, fallback: string | null): string | null {
  if (id === null) {
    return null;
  }
  try {
    return parseScopedId(id).engine ?? fallback;
  } catch {
    return fallback;
  }
}
