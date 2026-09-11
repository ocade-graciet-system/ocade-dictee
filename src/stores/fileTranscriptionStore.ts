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
} from "@/bindings";

// Le backend émet ce marqueur (français, indépendant de la langue UI) quand
// l'utilisateur annule via `cancel_file_transcription` — y compris pendant le
// téléchargement d'une URL. On le détecte pour présenter une annulation
// volontaire comme neutre plutôt que comme une erreur.
export const CANCELLED_MARKER = "annulée par l'utilisateur";

export type FileTranscriptionStatus = "idle" | "processing" | "done" | "error";

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
  formattedText: string | null;
  errorMessage: string | null;
  /** Id de l'entrée d'historique affichée (null si l'enregistrement a échoué). */
  historyId: number | null;
  formatting: boolean;
  cancelling: boolean;
  urlInput: string;
  history: FileHistoryItem[];
  initialized: boolean;

  initialize: () => Promise<void>;
  setUrlInput: (value: string) => void;
  startFile: (path: string) => Promise<void>;
  startUrl: (url: string) => Promise<void>;
  cancel: () => Promise<void>;
  reset: () => void;
  format: () => Promise<{ ok: true } | { ok: false; error: string }>;
  loadHistory: () => Promise<void>;
  openHistoryEntry: (id: number) => Promise<boolean>;
  deleteHistoryEntry: (id: number) => Promise<boolean>;
  downloadVideo: (
    id: number,
  ) => Promise<{ ok: true; path: string } | { ok: false; error: string }>;
  exportVideo: (id: number, destPath: string) => Promise<boolean>;
}

export const useFileTranscriptionStore = create<FileTranscriptionStore>()((
  set,
  get,
) => {
  const runTranscription = async (
    sourceLabel: string,
    sourceKind: "local" | "url",
    invoke: () => Promise<Result<FileTranscriptionResult, string>>,
  ): Promise<void> => {
    if (get().status === "processing") return;

    set({
      status: "processing",
      sourceLabel,
      sourceKind,
      progress: null,
      outputPath: null,
      markdown: null,
      formattedText: null,
      errorMessage: null,
      historyId: null,
      formatting: false,
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
    formattedText: null,
    errorMessage: null,
    historyId: null,
    formatting: false,
    cancelling: false,
    urlInput: "",
    history: [],
    initialized: false,

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
      await get().loadHistory();
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
        formattedText: null,
        errorMessage: null,
        historyId: null,
        formatting: false,
      });
    },

    format: async () => {
      const { markdown, formatting, historyId } = get();
      if (!markdown || formatting) {
        return { ok: false as const, error: "not ready" };
      }
      set({ formatting: true });
      try {
        const result = await commands.formatDocument(markdown);
        if (result.status !== "ok") {
          return { ok: false as const, error: result.error };
        }
        set({ formattedText: result.data });
        // Persiste la mise en forme dans l'historique pour la retrouver plus
        // tard ; un échec ici n'invalide pas le résultat affiché.
        if (historyId !== null) {
          const saved = await commands.fileHistoryUpdateFormatted(
            historyId,
            result.data,
          );
          if (saved.status !== "ok") {
            console.error("Failed to persist formatted text:", saved.error);
          }
        }
        return { ok: true as const };
      } catch (error) {
        return {
          ok: false as const,
          error: error instanceof Error ? error.message : String(error),
        };
      } finally {
        set({ formatting: false });
      }
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
      if (get().status === "processing") return false;
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
          formattedText: entry.formatted_text,
          historyId: entry.id,
          outputPath: null,
          errorMessage: null,
          progress: null,
          formatting: false,
        });
        return true;
      } catch (error) {
        console.error("Failed to open file history entry:", error);
        return false;
      }
    },

    deleteHistoryEntry: async (id) => {
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
  };
});
