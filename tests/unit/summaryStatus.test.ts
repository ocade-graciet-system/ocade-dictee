import { describe, expect, test } from "bun:test";
import {
  resolveSummaryStatus,
  SUMMARY_ACTIVE_STATUSES,
  type SummaryOutcome,
} from "../../src/lib/utils/summaryStatus";

describe("resolveSummaryStatus — résultat rattaché à l'entrée affichée", () => {
  test("succès : le compte-rendu est affiché", () => {
    expect(resolveSummaryStatus("done", false, null)).toBe("done");
  });
  test("échec : l'erreur est affichée", () => {
    expect(resolveSummaryStatus("error", false, null)).toBe("error");
  });
  test("annulation : retour à l'état précédent, sans message", () => {
    expect(resolveSummaryStatus("cancelled", false, null)).toBe("idle");
    expect(resolveSummaryStatus("cancelled", false, "# Compte-rendu")).toBe(
      "done",
    );
  });
});

describe("resolveSummaryStatus — entrée changée ou supprimée pendant le calcul", () => {
  test("succès écarté : retour à ce que l'entrée affichée montre", () => {
    expect(resolveSummaryStatus("done", true, null)).toBe("idle");
    expect(resolveSummaryStatus("done", true, "# Autre compte-rendu")).toBe(
      "done",
    );
  });
  test("échec écarté : pas d'erreur sur une entrée qui n'est pas la sienne", () => {
    expect(resolveSummaryStatus("error", true, null)).toBe("idle");
    expect(resolveSummaryStatus("error", true, "# Autre compte-rendu")).toBe(
      "done",
    );
  });
});

describe("resolveSummaryStatus — invariant", () => {
  // Le bug corrigé : l'entrée d'historique affichée est supprimée pendant le
  // résumé, `historyId` repasse à null, et le statut restait « summarizing »
  // pour toute la session (« Résumer », « Nouveau fichier » et l'historique
  // désactivés). Quelle que soit l'issue, le statut posé est terminal.
  const outcomes: SummaryOutcome[] = ["done", "error", "cancelled"];
  test("le statut posé n'est jamais un statut actif", () => {
    for (const outcome of outcomes) {
      for (const stale of [false, true]) {
        for (const shown of [null, "# Compte-rendu"]) {
          expect(SUMMARY_ACTIVE_STATUSES).not.toContain(
            resolveSummaryStatus(outcome, stale, shown),
          );
        }
      }
    }
  });
});
