import { create } from "zustand";
import { toast } from "sonner";
import i18n from "@/i18n";
import {
  commands,
  events,
  type FileHistoryItem,
  type FileTranscriptionProgress,
  type FileTranscriptionResult,
  type Result,
  type SummaryError,
  type SummaryProgress,
} from "@/bindings";
import {
  resolveSummaryStatus,
  SUMMARY_ACTIVE_STATUSES,
  type SummaryOutcome,
  type SummaryStatus,
} from "@/lib/utils/summaryStatus";

// Le backend émet ce marqueur (français, indépendant de la langue UI) quand
// l'utilisateur annule via `cancel_file_transcription` — y compris pendant le
// téléchargement d'une URL. On le détecte pour présenter une annulation
// volontaire comme neutre plutôt que comme une erreur.
export const CANCELLED_MARKER = "annulée par l'utilisateur";

export type FileTranscriptionStatus = "idle" | "processing" | "done" | "error";

// Le vocabulaire des statuts du compte-rendu vit dans un module pur (testable
// sans Tauri) ; il reste exposé ici, seul import de ses consommateurs.
export { SUMMARY_ACTIVE_STATUSES, type SummaryStatus };

const getFileName = (path: string): string => {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
};

// Tout l'état de la page Fichier vit ici (et non dans le composant) : il
// survit aux changements d'onglet, et l'écouteur de progression enregistré par
// `initialize()` continue de suivre une transcription en cours même quand la
// page est démontée. Rien ne se perd.
interface FileTranscriptionStore {
  status: FileTranscriptionStatus;
  /** Nom du fichier ou URL, pour l'affichage. */
  sourceLabel: string | null;
  /** Type de la source affichée ("local" | "url"), pour les actions dédiées. */
  sourceKind: "local" | "url" | null;
  /** Téléchargements de vidéo en cours : id d'historique -> pourcentage. */
  videoDownloads: Record<number, number>;
  progress: FileTranscriptionProgress | null;
  /** Chemin du `.md` écrit sur disque (null pour une entrée re-consultée). */
  outputPath: string | null;
  markdown: string | null;
  errorMessage: string | null;
  /** Id de l'entrée d'historique affichée (null si l'enregistrement a échoué). */
  historyId: number | null;
  cancelling: boolean;
  urlInput: string;
  history: FileHistoryItem[];
  initialized: boolean;

  /** Compte-rendu local (plan 09). */
  summaryStatus: SummaryStatus;
  summaryProgress: SummaryProgress | null;
  /** Le résumé en cours a d'abord besoin d'un téléchargement (moteur/modèle). */
  summaryNeedsDownload: boolean;
  /** Moteur et modèle déjà installés (null tant que non interrogé). */
  summaryInstalled: boolean | null;
  /** Octets restant à télécharger : taille totale des éléments manquants
   *  (moteur et/ou modèle), pour annoncer le téléchargement unique. */
  summaryDownloadBytes: number;
  summaryMarkdown: string | null;
  summaryError: SummaryError | null;
  /** Annulation du résumé demandée : le bouton « Annuler » est désarmé. */
  cancellingSummary: boolean;
  /** Annulation demandée avant que le moteur n'ait démarré : elle serait sinon
   *  perdue (rien à annuler côté moteur) et le résumé partirait quand même. */
  summaryCancelRequested: boolean;

  initialize: () => Promise<void>;
  setUrlInput: (value: string) => void;
  startFile: (path: string) => Promise<void>;
  startUrl: (url: string) => Promise<void>;
  cancel: () => Promise<void>;
  reset: () => void;
  loadHistory: () => Promise<void>;
  openHistoryEntry: (id: number) => Promise<boolean>;
  deleteHistoryEntry: (id: number) => Promise<boolean>;
  downloadVideo: (
    id: number,
  ) => Promise<{ ok: true; path: string } | { ok: false; error: string }>;
  exportVideo: (id: number, destPath: string) => Promise<boolean>;
  refreshSummaryStatus: () => Promise<void>;
  summarize: () => Promise<void>;
  cancelSummary: () => Promise<void>;
}

// Remis à zéro à chaque changement de document affiché. `summaryInstalled` et
// `summaryDownloadBytes` n'en font pas partie : ce sont des faits
// d'installation, indépendants du document consulté.
const EMPTY_SUMMARY = {
  summaryStatus: "idle" as SummaryStatus,
  summaryProgress: null,
  summaryNeedsDownload: false,
  summaryMarkdown: null,
  summaryError: null,
  cancellingSummary: false,
  summaryCancelRequested: false,
};

export const useFileTranscriptionStore = create<FileTranscriptionStore>()((
  set,
  get,
) => {
  // Transcription ou compte-rendu en cours : le moteur est occupé, et toute
  // action qui remplacerait ou supprimerait le document affiché est refusée
  // (l'interface désactive déjà ces commandes ; ceci couvre les courses, par
  // exemple une confirmation de suppression ouverte avant le résumé).
  const isBusy = (): boolean => {
    const { status, summaryStatus } = get();
    return (
      status === "processing" || SUMMARY_ACTIVE_STATUSES.includes(summaryStatus)
    );
  };

  const runTranscription = async (
    sourceLabel: string,
    sourceKind: "local" | "url",
    invoke: () => Promise<Result<FileTranscriptionResult, string>>,
  ): Promise<void> => {
    if (isBusy()) return;

    set({
      status: "processing",
      sourceLabel,
      sourceKind,
      progress: null,
      outputPath: null,
      markdown: null,
      errorMessage: null,
      historyId: null,
      ...EMPTY_SUMMARY,
    });

    try {
      const result = await invoke();
      if (result.status !== "ok") {
        throw new Error(result.error);
      }
      set({
        outputPath: result.data.path,
        markdown: result.data.content,
        historyId: result.data.history_id,
        status: "done",
      });
      void get().loadHistory();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      console.error("File transcription failed:", message);
      if (message.includes(CANCELLED_MARKER)) {
        toast.message(i18n.t("settings.file.cancelled"));
        set({ status: "idle", progress: null });
      } else {
        set({ status: "error", errorMessage: message });
      }
    } finally {
      set({ cancelling: false });
    }
  };

  return {
    status: "idle",
    sourceLabel: null,
    sourceKind: null,
    videoDownloads: {},
    progress: null,
    outputPath: null,
    markdown: null,
    errorMessage: null,
    historyId: null,
    cancelling: false,
    urlInput: "",
    history: [],
    initialized: false,
    ...EMPTY_SUMMARY,
    summaryInstalled: null,
    summaryDownloadBytes: 0,

    initialize: async () => {
      if (get().initialized) return;
      set({ initialized: true });
      // Écouteur au niveau du store : la progression reste suivie même si la
      // page Fichier est démontée pendant une transcription.
      await events.fileTranscriptionProgress.listen((event) => {
        set({ progress: event.payload });
      });
      await events.videoDownloadProgress.listen((event) => {
        set((state) => ({
          videoDownloads: {
            ...state.videoDownloads,
            [event.payload.history_id]: event.payload.percent,
          },
        }));
      });
      // Progression du compte-rendu : les phases de téléchargement gardent le
      // statut « preparing », le démarrage et le résumé ont le leur. Un
      // événement tardif — arrivé après une annulation, une erreur ou la fin —
      // est ignoré : il ferait réapparaître l'encart d'un résumé terminé.
      await events.summaryProgress.listen((event) => {
        if (!SUMMARY_ACTIVE_STATUSES.includes(get().summaryStatus)) return;
        const { phase } = event.payload;
        set({
          summaryProgress: event.payload,
          summaryStatus:
            phase === "starting"
              ? "starting"
              : phase === "summarizing"
                ? "summarizing"
                : "preparing",
        });
      });
      await get().loadHistory();
      await get().refreshSummaryStatus();
    },

    setUrlInput: (value) => set({ urlInput: value }),

    startFile: async (path) => {
      await runTranscription(getFileName(path), "local", () =>
        commands.transcribeAudioFile(path),
      );
    },

    startUrl: async (url) => {
      await runTranscription(url, "url", () => commands.transcribeUrl(url));
    },

    cancel: async () => {
      set({ cancelling: true });
      try {
        await commands.cancelFileTranscription();
      } catch (error) {
        console.error("Failed to cancel file transcription:", error);
        set({ cancelling: false });
      }
    },

    reset: () => {
      set({
        status: "idle",
        sourceLabel: null,
        sourceKind: null,
        progress: null,
        outputPath: null,
        markdown: null,
        errorMessage: null,
        historyId: null,
        ...EMPTY_SUMMARY,
      });
    },

    loadHistory: async () => {
      try {
        const result = await commands.fileHistoryList();
        if (result.status === "ok") {
          set({ history: result.data });
        } else {
          console.error("Failed to load file history:", result.error);
        }
      } catch (error) {
        console.error("Failed to load file history:", error);
      }
    },

    openHistoryEntry: async (id) => {
      if (isBusy()) return false;
      try {
        const result = await commands.fileHistoryGet(id);
        if (result.status !== "ok" || result.data === null) {
          return false;
        }
        const entry = result.data;
        set({
          status: "done",
          sourceLabel: entry.source_name,
          sourceKind: entry.source_kind === "url" ? "url" : "local",
          markdown: entry.raw_text,
          historyId: entry.id,
          outputPath: null,
          errorMessage: null,
          progress: null,
          ...EMPTY_SUMMARY,
          // Compte-rendu enregistré : affiché sans recalcul.
          summaryMarkdown: entry.summary_markdown,
          summaryStatus: entry.summary_markdown ? "done" : "idle",
        });
        return true;
      } catch (error) {
        console.error("Failed to open file history entry:", error);
        return false;
      }
    },

    deleteHistoryEntry: async (id) => {
      if (isBusy()) return false;
      try {
        const result = await commands.fileHistoryDelete(id);
        if (result.status !== "ok") {
          console.error("Failed to delete file history entry:", result.error);
          return false;
        }
        if (get().historyId === id) {
          set({ historyId: null });
        }
        await get().loadHistory();
        return true;
      } catch (error) {
        console.error("Failed to delete file history entry:", error);
        return false;
      }
    },

    downloadVideo: async (id) => {
      if (get().videoDownloads[id] !== undefined) {
        return { ok: false as const, error: "already downloading" };
      }
      set((state) => ({
        videoDownloads: { ...state.videoDownloads, [id]: 0 },
      }));
      try {
        const result = await commands.downloadEntryVideo(id);
        if (result.status !== "ok") {
          return { ok: false as const, error: result.error };
        }
        // La liste reflète le nouveau has_video de l'entrée.
        void get().loadHistory();
        return { ok: true as const, path: result.data };
      } catch (error) {
        return {
          ok: false as const,
          error: error instanceof Error ? error.message : String(error),
        };
      } finally {
        set((state) => {
          const videoDownloads = { ...state.videoDownloads };
          delete videoDownloads[id];
          return { videoDownloads };
        });
      }
    },

    exportVideo: async (id, destPath) => {
      try {
        const result = await commands.exportEntryVideo(id, destPath);
        if (result.status !== "ok") {
          console.error("Failed to export video:", result.error);
          return false;
        }
        return true;
      } catch (error) {
        console.error("Failed to export video:", error);
        return false;
      }
    },

    refreshSummaryStatus: async () => {
      try {
        const status = await commands.summaryStatus();
        set({
          summaryInstalled: status.engineReady && status.modelReady,
          summaryDownloadBytes: status.downloadSizeBytes,
        });
      } catch (error) {
        console.error("Failed to read summary status:", error);
      }
    },

    summarize: async () => {
      const { markdown, historyId, summaryStatus, summaryInstalled } = get();
      if (!markdown || SUMMARY_ACTIVE_STATUSES.includes(summaryStatus)) return;

      // « preparing » posé d'abord, sans attente : le bouton se désactive
      // immédiatement et un second clic ne peut pas lancer un résumé
      // concurrent (que le moteur refuserait par `Busy`).
      set({
        summaryStatus: "preparing",
        summaryProgress: null,
        summaryNeedsDownload: summaryInstalled === false,
        summaryError: null,
        cancellingSummary: false,
        summaryCancelRequested: false,
      });

      // L'entrée affichée peut changer pendant le calcul : réouverture d'une
      // autre entrée d'historique, ou suppression de celle-ci (`historyId`
      // repasse alors à null). Le compte-rendu et l'erreur, rattachés à
      // l'entrée d'origine, ne sont dans ce cas pas affichés — le compte-rendu
      // est de toute façon enregistré côté backend pour `historyId`.
      const target = historyId;
      const stillCurrent = () => get().historyId === target;

      // Le statut terminal, lui, est posé quoi qu'il arrive : le conditionner
      // à `stillCurrent()` laissait l'interface bloquée en « résumé en cours »
      // pour toute la session dès que l'entrée affichée disparaissait.
      const settle = (
        outcome: SummaryOutcome,
        attached: {
          summaryMarkdown?: string;
          summaryError?: SummaryError;
        } = {},
      ): void => {
        const stale = !stillCurrent();
        set({
          ...(stale ? {} : attached),
          summaryStatus: resolveSummaryStatus(
            outcome,
            stale,
            get().summaryMarkdown,
          ),
          summaryProgress: null,
          cancellingSummary: false,
          summaryCancelRequested: false,
        });
      };

      // Annonce du téléchargement unique si le moteur ou le modèle manque.
      // Statut illisible : on garde la dernière valeur connue (celle
      // d'`initialize`) plutôt que de ne rien annoncer.
      await get().refreshSummaryStatus();
      set({ summaryNeedsDownload: get().summaryInstalled === false });

      // « Annuler » cliqué pendant cette lecture d'état : le moteur n'a encore
      // rien commencé, la demande y serait sans effet et le résumé partirait
      // malgré le clic. Il suffit de ne pas le lancer.
      if (get().summaryCancelRequested) {
        settle("cancelled");
        return;
      }

      try {
        const result = await commands.summarizeDocument(markdown, historyId);
        if (result.status === "ok") {
          set({ summaryInstalled: true, summaryDownloadBytes: 0 });
          settle("done", { summaryMarkdown: result.data });
          void get().loadHistory();
          return;
        }
        if (result.error.kind === "cancelled") {
          // Retour silencieux à l'état précédent, sans message.
          settle("cancelled");
          return;
        }
        settle("error", { summaryError: result.error });
      } catch (error) {
        console.error("Summary failed:", error);
        settle("error", {
          summaryError: {
            kind: "engineStartFailed",
            detail: error instanceof Error ? error.message : String(error),
          },
        });
      }
    },

    cancelSummary: async () => {
      // Comme pour la transcription : le bouton est désarmé dès le clic, puis
      // réarmé à la résolution du résumé (`settle`) — ou tout de suite si la
      // demande d'annulation elle-même a échoué. L'intention est mémorisée :
      // cliquée avant que `summarizeDocument` ne soit appelée, l'annulation
      // n'a rien à annuler côté moteur et serait autrement perdue.
      set({ cancellingSummary: true, summaryCancelRequested: true });
      try {
        await commands.cancelSummary();
      } catch (error) {
        console.error("Failed to cancel summary:", error);
        set({ cancellingSummary: false });
      }
    },
  };
});
