/** An engine discovered for the authenticated browser account. */
export interface OwnedEngine {
  /** Opaque registry identity; never parse it as a URL. */
  readonly key: string;
  /** Cookie-authenticated relay transport URL, independent of identity. */
  readonly endpoint: string;
  readonly label: string;
  /** Expected identity verified by EngineInfo before publishing rows. */
  readonly deviceId: string;
}

/** The owned engines available to the browser's current session. */
export interface FleetState {
  readonly active: string | null;
  readonly engines: readonly OwnedEngine[];
  readonly configurationError: string | null;
}
