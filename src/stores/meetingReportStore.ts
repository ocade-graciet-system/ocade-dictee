import { create } from "zustand";
import { toast } from "sonner";
import i18n from "@/i18n";
import {
  commands,
  events,
  type MeetingError,
  type MeetingReport,
  type MeetingReportProgress,
} from "@/bindings";
import { useMeetingStore } from "./meetingStore";

interface ReportJob {
  recordingId: number;
  progress: MeetingReportProgress | null;
  cancelling: boolean;
}

// Compte rendu de réunion : comme l'enregistrement, son état vit dans un store
// (et non dans le composant) pour survivre aux changements d'onglet — une
// rédaction dure plusieurs minutes.
interface MeetingReportStore {
  /** Enregistrement dont le compte rendu est affiché. */
  openId: number | null;
  reports: Record<number, MeetingReport>;
  job: ReportJob | null;
  /** Dernière erreur par enregistrement. */
  errors: Record<number, MeetingError>;
  /** Moteur et modèle de rédaction déjà installés (null : pas encore lu). */
  engineInstalled: boolean | null;
  /** Octets restant à télécharger au premier compte rendu. */
  downloadBytes: number;
  initialized: boolean;

  initialize: () => Promise<void>;
  open: (id: number) => Promise<void>;
  close: () => void;
  loadReport: (id: number) => Promise<void>;
  generate: (id: number, retranscribe?: boolean) => Promise<void>;
  cancel: () => Promise<void>;
  refreshEngineStatus: () => Promise<void>;
  /** Oublie un enregistrement supprimé. */
  forget: (id: number) => void;
}

export const useMeetingReportStore = create<MeetingReportStore>()(
  (set, get) => ({
    openId: null,
    reports: {},
    job: null,
    errors: {},
    engineInstalled: null,
    downloadBytes: 0,
    initialized: false,

    initialize: async () => {
      if (get().initialized) return;
      set({ initialized: true });
      await events.meetingReportProgress.listen((event) => {
        const progress = event.payload;
        const job = get().job;
        // Rédaction lancée avant un rechargement de la fenêtre : on la suit
        // quand même, et on relit le résultat à la fin.
        if (!job) {
          set({
            job: {
              recordingId: progress.recordingId,
              progress,
              cancelling: false,
            },
          });
        } else if (job.recordingId === progress.recordingId) {
          set({ job: { ...job, progress } });
        }
      });
      void get().refreshEngineStatus();
    },

    open: async (id) => {
      set({ openId: id });
      await get().loadReport(id);
    },

    close: () => set({ openId: null }),

    loadReport: async (id) => {
      const result = await commands.meetingGetReport(id);
      if (result.status === "ok") {
        set({ reports: { ...get().reports, [id]: result.data } });
      }
    },

    generate: async (id, retranscribe = false) => {
      if (get().job) return;
      const { [id]: _previous, ...errors } = get().errors;
      set({
        job: { recordingId: id, progress: null, cancelling: false },
        errors,
      });
      try {
        const result = await commands.meetingGenerateReport(id, retranscribe);
        if (result.status === "ok") {
          set({ reports: { ...get().reports, [id]: result.data } });
          toast.success(i18n.t("settings.meeting.report.ready"));
          void useMeetingStore.getState().loadRecordings();
        } else if (result.error.kind === "cancelled") {
          toast.info(i18n.t("settings.meeting.report.cancelled"));
        } else {
          set({ errors: { ...get().errors, [id]: result.error } });
        }
      } finally {
        set({ job: null });
        // La transcription a pu être faite même si la rédaction a échoué.
        void get().loadReport(id);
        void get().refreshEngineStatus();
      }
    },

    cancel: async () => {
      const job = get().job;
      if (!job || job.cancelling) return;
      set({ job: { ...job, cancelling: true } });
      await commands.meetingCancelReport();
    },

    refreshEngineStatus: async () => {
      try {
        const status = await commands.summaryStatus();
        set({
          engineInstalled: status.engineReady && status.modelReady,
          downloadBytes: status.downloadSizeBytes,
        });
      } catch (error) {
        console.error("Failed to read summary engine status:", error);
      }
    },

    forget: (id) => {
      const { [id]: _report, ...reports } = get().reports;
      const { [id]: _error, ...errors } = get().errors;
      set({
        reports,
        errors,
        openId: get().openId === id ? null : get().openId,
      });
    },
  }),
);
