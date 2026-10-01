/** A session generation, not an owner/device ID. Capture before async work. */
let generation = 0;

export function privateSessionGeneration(): number {
  return generation;
}

export function isCurrentPrivateSession(generationAtStart: number): boolean {
  return generationAtStart === generation;
}

/** Called only by the private-session lifecycle boundary. */
export function advancePrivateSessionGeneration(): void {
  generation += 1;
}
