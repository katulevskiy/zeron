import type { EngineStatus } from "@zeron/engine-client";
import { useMemo } from "react";

export interface ConnectionStateView {
  readonly className: string;
  readonly dot: string;
  readonly label: string;
  readonly detail: string | null;
  readonly parked: boolean;
  readonly pairable: boolean;
}

export function connectionState(status: EngineStatus | null): ConnectionStateView {
  if (status === null) {
    return { className: "conn-connecting", dot: "dot-connecting", label: "Connecting…", detail: null, parked: false, pairable: false };
  }
  switch (status.state) {
    case "connecting":
      return { className: "conn-connecting", dot: "dot-connecting", label: "Connecting…", detail: `Attempt ${status.attempt}`, parked: false, pairable: false };
    case "connected":
      return { className: "conn-connected", dot: "dot-connected", label: "Connected", detail: null, parked: false, pairable: false };
    case "reconnecting":
      return {
        className: "conn-reconnecting",
        dot: "dot-reconnecting",
        label: "Reconnecting…",
        detail: status.lastError,
        parked: false,
        pairable: false,
      };
    case "offline":
      return { className: "conn-closed", dot: "dot-closed", label: "Engine offline", detail: "Cached history is read-only until the host returns.", parked: false, pairable: false };
    case "parked":
      return {
        className: "conn-parked",
        dot: "dot-parked",
        label: status.reason === "identity-changed" ? "Engine changed" : "Session revoked",
        detail: status.detail,
        parked: true,
        pairable: false,
      };
    case "closed":
      return { className: "conn-closed", dot: "dot-closed", label: "Disconnected", detail: null, parked: false, pairable: false };
  }
}

export function useConnectionState(status: EngineStatus | null): ConnectionStateView {
  return useMemo(() => connectionState(status), [status]);
}
