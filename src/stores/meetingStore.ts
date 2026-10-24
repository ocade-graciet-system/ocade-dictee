import { create } from "zustand";
import { toast } from "sonner";
import i18n from "@/i18n";
import {
  commands,
  events,
  type MeetingAssets,
  type MeetingError,
  type MeetingRecorderStatus,
  type MeetingRecording,
  type Result,
} from "@/bindings";
import {
  formatDuration,
  meetingErrorMessage,
} from "@/components/meeting/format";

const IDLE_STATUS: MeetingRecorderStatus = {
  phase: "idle",
  recordingId: null,
  durationMs: 0,
  sizeBytes: 0,
  level: 0,
  microphone: null,
};

type RecorderAction = "start" | "pause" | "resume" | "stop";

// L'état de l'onglet Réunion vit ici (et non dans le composant) : un
// enregistrement continue quand on change d'onglet ou qu'on ferme la fenêtre
// (l'application reste dans la barre des tâches), et l'interface doit le
// retrouver tel quel en revenant.
interface MeetingStore {
  status: MeetingRecorderStatus;
  recordings: MeetingRecording[];
  assets: MeetingAssets | null;
  /** Action en cours sur l'enregistreur : ses boutons sont désarmés. */
  pendingAction: RecorderAction | null;
  /** Dernier enregistrement terminé, mis en évidence dans la liste. */
  highlightedId: number | null;
  initialized: boolean;

  initialize: () => Promise<void>;
  loadRecordings: () => Promise<void>;
  loadAssets: () => Promise<void>;
  start: () => Promise<MeetingError | null>;
  pause: () => Promise<MeetingError | null>;
  resume: () => Promise<MeetingError | null>;
  stop: () => Promise<MeetingRecording | null>;
  rename: (id: number, name: string) => Promise<MeetingError | null>;
  remove: (id: number) => Promise<MeetingError | null>;
  /** Applique le résultat d'une commande sur les éléments du compte rendu. */
  applyAssets: (result: Result<MeetingAssets, MeetingError>) => boolean;
}

const showError = (error: MeetingError) => {
  toast.error(meetingErrorMessage(i18n.t, error));
};

export const useMeetingStore = create<MeetingStore>()((set, get) => {
  const runRecorderAction = async (
    action: RecorderAction,
    call: () => Promise<Result<MeetingRecorderStatus, MeetingError>>,
  ): Promise<MeetingError | null> => {
    if (get().pendingAction) return null;
    set({ pendingAction: action });
    try {
      const result = await call();
      if (result.status === "error") return result.error;
      set({ status: result.data });
      return null;
    } finally {
      set({ pendingAction: null });
      void get().loadRecordings();
    }
  };

  return {
    status: IDLE_STATUS,
    recordings: [],
    assets: null,
    pendingAction: null,
    highlightedId: null,
    initialized: false,

    initialize: async () => {
      if (get().initialized) return;
      set({ initialized: true });

      // Écouteurs posés une seule fois pour toute la vie de l'application.
      await events.meetingRecorderStatus.listen((event) => {
        const previous = get().status;
        set({ status: event.payload });
        // Changement de phase (pause, micro perdu…) : la liste reflète l'état.
        if (previous.phase !== event.payload.phase) {
          void get().loadRecordings();
        }
      });
      await events.meetingRecordingFinished.listen((event) => {
        const { recording, reason } = event.payload;
        set({ highlightedId: recording.id });
        void get().loadRecordings();
        // Arrêt décidé par l'application, pas par l'utilisateur : il faut le
        // lui dire, l'enregistrement ne continue plus.
        if (reason === "diskFull") {
          toast.error(i18n.t("settings.meeting.recorder.stoppedDiskFull"), {
            duration: Infinity,
          });
        } else if (reason === "writeFailed") {
          toast.error(i18n.t("settings.meeting.recorder.stoppedWriteFailed"), {
            duration: Infinity,
          });
        }
      });

      try {
        set({ status: await commands.meetingRecorderStatus() });
      } catch (error) {
        console.error("Failed to read meeting recorder status:", error);
      }
      await Promise.all([get().loadRecordings(), get().loadAssets()]);
    },

    loadRecordings: async () => {
      const result = await commands.meetingList();
      if (result.status === "ok") {
        set({ recordings: result.data });
      } else {
        console.error("Failed to list meeting recordings:", result.error);
      }
    },

    loadAssets: async () => {
      const result = await commands.meetingAssets();
      if (result.status === "ok") {
        set({ assets: result.data });
      } else {
        console.error("Failed to load meeting assets:", result.error);
      }
    },

    start: () => {
      set({ highlightedId: null });
      return runRecorderAction("start", commands.meetingStart);
    },
    pause: () => runRecorderAction("pause", commands.meetingPause),
    resume: () => runRecorderAction("resume", commands.meetingResume),

    stop: async () => {
      if (get().pendingAction) return null;
      set({ pendingAction: "stop" });
      try {
        const result = await commands.meetingStop();
        if (result.status === "error") {
          showError(result.error);
          return null;
        }
        const recording = result.data;
        set({ status: IDLE_STATUS, highlightedId: recording.id });
        toast.success(i18n.t("settings.meeting.recorder.finished"), {
          description: i18n.t("settings.meeting.recorder.finishedDetail", {
            name: recording.name,
            duration: formatDuration(recording.durationMs),
          }),
        });
        return recording;
      } finally {
        set({ pendingAction: null });
        void get().loadRecordings();
      }
    },

    rename: async (id, name) => {
      const result = await commands.meetingRename(id, name);
      if (result.status === "error") return result.error;
      set({
        recordings: get().recordings.map((r) =>
          r.id === id ? { ...r, name: name.trim().replace(/\s+/g, " ") } : r,
        ),
      });
      return null;
    },

    remove: async (id) => {
      const result = await commands.meetingDelete(id);
      if (result.status === "error") return result.error;
      set({
        recordings: get().recordings.filter((r) => r.id !== id),
        highlightedId: get().highlightedId === id ? null : get().highlightedId,
      });
      return null;
    },

    applyAssets: (result) => {
      if (result.status === "error") {
        showError(result.error);
        return false;
      }
      set({ assets: result.data });
      return true;
    },
  };
});

/** Un enregistrement est en cours ou en pause (la mise à jour automatique
 *  attend, elle redémarrerait l'application). */
export const isMeetingRecorderActive = (status: MeetingRecorderStatus) =>
  status.phase !== "idle";
