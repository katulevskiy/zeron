import { useEffect, type Dispatch, type SetStateAction } from "react";
import type { HarnessDescriptor, Model } from "@zeron/proto";
import type { DraftConfig } from "./composer-actions";
import { rememberedModelFor, reconcileDraftModel, reconcileFreshDraftModel } from "./composer-draft";

/**
 * The composer's draft↔catalog reconciliation (pickers.rs:713-796's sticky
 * seeding + 1493-1512's descriptor-aware normalization) as ONE effect the
 * composer wires with its live catalog rows — extracted from composer.tsx
 * so the mounted suite can exercise the real owner (ticket 77). Fresh drafts
 * additionally discard options not offered by their selected engine; existing
 * chats retain their committed option picks.
 *
 * Both live inputs are observed: the harness's model rows AND the harness
 * catalog rows, so a descriptor that lands after the models (or an
 * equivalent refresh of either) re-derives the selection against the
 * effective ladder. `reconcileDraftModel` returns the prior draft when
 * nothing changed, so the effect cannot setState-loop, and a stored
 * preference survives while the effective ladder is still empty.
 */
export function useDraftModelReconciliation(
  models: readonly Model[],
  harnesses: readonly HarnessDescriptor[],
  setDraft: Dispatch<SetStateAction<DraftConfig>>,
  fresh = false,
): void {
  useEffect(() => {
    if (models.length === 0) {
      return;
    }
    setDraft((current) => {
      const descriptor = harnesses.find((row) => row.id === current.harness) ?? null;
      const reconcile = fresh ? reconcileFreshDraftModel : reconcileDraftModel;
      return reconcile(current, models, descriptor, rememberedModelFor(current.harness));
    });
  }, [models, harnesses, setDraft, fresh]);
}
