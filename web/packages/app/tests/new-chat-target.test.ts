import { describe, expect, it } from "vitest";
import { encodeScopedId } from "@zeron/engine-client";
import {
  targetForDevicePick,
  targetForProjectPick,
  type NewChatDefaults,
} from "../src/lib/new-chat-target";

const ONE = "engine_one";
const TWO = "engine_two";
const deviceOne = encodeScopedId(ONE, "device-one");
const deviceTwo = encodeScopedId(TWO, "device-two");
const projectOne = encodeScopedId(ONE, "project-one");
const projectTwo = encodeScopedId(TWO, "project-two");

function defaults(overrides: Partial<NewChatDefaults> = {}): NewChatDefaults {
  return { device: deviceOne, project: projectOne, noProject: false, ...overrides };
}

describe("new-chat engine and project selection policy", () => {
  it("clears a foreign project and records an explicit projectless target when switching engines", () => {
    expect(targetForDevicePick(defaults(), deviceTwo)).toEqual({ device: deviceTwo, project: null, noProject: true });
  });

  it("preserves the selected project when the selected device has the same owner", () => {
    expect(targetForDevicePick(defaults(), deviceOne)).toEqual({ device: deviceOne, project: projectOne, noProject: false });
  });

  it("choosing a project selects that project's owning device", () => {
    expect(targetForProjectPick(projectTwo, deviceTwo)).toEqual({ device: deviceTwo, project: projectTwo, noProject: false });
  });
});
