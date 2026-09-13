/**
 * États du compte-rendu et résolution de l'état terminal (spec plan 09, §5).
 * Module pur — ni Tauri, ni i18n, ni store — donc testable directement
 * (`tests/unit/summaryStatus.test.ts`). Réexporté par
 * `stores/fileTranscriptionStore` : les consommateurs gardent un seul import.
 */

/** États du compte-rendu (spec plan 09, §5). */
export type SummaryStatus =
  "idle" | "preparing" | "starting" | "summarizing" | "done" | "error";

/** Statuts pendant lesquels un résumé occupe le moteur : bouton désactivé,
 *  actions destructrices verrouillées, mise à jour forcée reportée. */
export const SUMMARY_ACTIVE_STATUSES: readonly SummaryStatus[] = [
  "preparing",
  "starting",
  "summarizing",
];

/** Issue d'un appel à `summarize_document`. */
export type SummaryOutcome = "done" | "error" | "cancelled";

/**
 * Statut terminal à poser à la résolution d'un résumé — **toujours**, même
 * quand le résultat ne concerne plus l'entrée affichée (`stale` : une autre
 * entrée a été ouverte, ou l'entrée a été supprimée pendant le calcul, ce qui
 * remet `historyId` à null). Sans cela l'interface resterait bloquée en
 * « résumé en cours » pour toute la session : « Résumer », « Nouveau fichier »
 * et l'historique désactivés, mise à jour forcée reportée indéfiniment.
 *
 * `shownSummary` est le compte-rendu de l'entrée actuellement affichée :
 * quand le résultat est écarté (annulation, ou `stale`), on revient
 * simplement à ce qu'elle montre déjà.
 */
export function resolveSummaryStatus(
  outcome: SummaryOutcome,
  stale: boolean,
  shownSummary: string | null,
): SummaryStatus {
  if (outcome === "cancelled" || stale) return shownSummary ? "done" : "idle";
  return outcome;
}
