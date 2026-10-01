import { describe, expect, it } from "vitest";
import { UiSettingsStore, UI_SETTINGS_STORAGE_KEY } from "../src/state/ui-settings";
import { ComposerDefaultsStore } from "../src/lib/composer-draft";

function storage() {
  const values = new Map<string, string>();
  return { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); }, removeItem: (key: string) => { values.delete(key); } };
}

describe("private settings retention policy", () => {
  it("keeps sidebar session IDs only in this authenticated window, while retaining customization", () => {
    const disk = storage();
    const settings = new UiSettingsStore({ storage: disk });
    settings.update({ sidebarWidth: 300, lastSpaceId: "owner-a/project", spaceFilter: "owner-a/project", lastProjectActionBySpaceId: { "owner-a/project": "private-command" }, sidebarPinnedSessionIdsByProfile: { a: ["private-chat"] }, sidebarSectionsByProfile: { a: [{ id: "section", name: "Secret customer", sessionIds: ["private-chat"], collapsed: false }] } });
    expect(settings.getSnapshot().lastSpaceId).toBe("owner-a/project");
    const reloaded = new UiSettingsStore({ storage: disk }).getSnapshot();
    expect(reloaded.lastSpaceId).toBeNull();
    expect(reloaded.spaceFilter).toBeNull();
    expect(reloaded.lastProjectActionBySpaceId).toEqual({});
    expect(reloaded.sidebarPinnedSessionIdsByProfile).toEqual({});
    expect(reloaded.sidebarSectionsByProfile).toEqual({});
    expect(reloaded.sidebarWidth).toBe(300);
    expect(disk.getItem(UI_SETTINGS_STORAGE_KEY)).not.toContain("private-chat");
  });

  it("does not reload the previous owner's device/project defaults but keeps model preferences", () => {
    const disk = storage();
    const settings = new ComposerDefaultsStore({ storage: disk });
    settings.update({ device: "owner-a-device", project: "owner-a-path", noProject: true, harness: "codex", modelByHarness: { codex: { id: "model", label: "Preferred model" } } });
    const reloaded = new ComposerDefaultsStore({ storage: disk }).getSnapshot();
    expect(reloaded.device).toBeNull();
    expect(reloaded.project).toBeNull();
    expect(reloaded.noProject).toBe(false);
    expect(reloaded.harness).toBe("codex");
    expect(reloaded.modelByHarness).toEqual({ codex: { id: "model", label: "Preferred model" } });
  });
});
