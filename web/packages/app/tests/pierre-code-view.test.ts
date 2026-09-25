// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CodeView } from "../src/components/files/code-view";

vi.mock("../src/state/appearance", () => ({ useResolvedAppearance: () => "dark" }));
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const containers: HTMLDivElement[] = [];
afterEach(() => {
  for (const container of containers.splice(0)) container.remove();
});

describe("Pierre file editor", () => {
  it("mounts an editable file without the old transparent textarea", async () => {
    const container = document.createElement("div");
    containers.push(container);
    document.body.append(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(createElement(CodeView, {
        text: "const answer = 42;\n",
        path: "answer.ts",
        editable: true,
        onChange: () => {},
        codeFontSize: 13,
        wordWrap: true,
      }));
    });
    expect(container.querySelector("diffs-container")).not.toBeNull();
    expect(container.querySelector("textarea.files-code-input")).toBeNull();
    await act(async () => { root.unmount(); });
  });
});
