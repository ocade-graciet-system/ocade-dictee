import React from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ask } from "@tauri-apps/plugin-dialog";
import { Loader2, Mic, Pause, Play, Square } from "lucide-react";
import { useMeetingStore } from "@/stores/meetingStore";
import { useSettings } from "@/hooks/useSettings";
import { Button } from "../ui/Button";
import { Alert } from "../ui/Alert";
import { formatClock, formatSize, meetingErrorMessage } from "./format";

// Barre de niveau du micro : la preuve, pour l'utilisateur, que sa voix est
// bien captée.
const LevelMeter: React.FC<{ level: number }> = ({ level }) => (
  <div
    className="h-2 w-full max-w-xs overflow-hidden rounded-full bg-mid-gray/20"
    aria-hidden="true"
  >
    <div
      className="h-full rounded-full bg-logo-primary transition-[width] duration-200 ease-out"
      style={{ width: `${Math.round(level * 100)}%` }}
    />
  </div>
);

export const RecorderPanel: React.FC = () => {
  const { t } = useTranslation();
  const { status, recordings, pendingAction, start, pause, resume, stop } =
    useMeetingStore();
  const { getSetting } = useSettings();

  const phase = status.phase;
  const current = recordings.find((r) => r.id === status.recordingId);
  const selectedMicrophone = getSetting("selected_microphone");
  const settingsMicrophone =
    !selectedMicrophone || selectedMicrophone.toLowerCase() === "default"
      ? t("settings.meeting.recorder.defaultMicrophone")
      : selectedMicrophone;

  const reportError = (error: Parameters<typeof meetingErrorMessage>[1]) => {
    toast.error(meetingErrorMessage(t, error));
  };

  const handleStart = async () => {
    const error = await start();
    if (error) reportError(error);
  };

  const handlePause = async () => {
    const error = await pause();
    if (error) reportError(error);
  };

  const handleResume = async () => {
    const error = await resume();
    if (error) reportError(error);
  };

  // « Terminer » est définitif : une personne qui voulait faire une pause ne
  // doit pas clore sa réunion par erreur.
  const handleStop = async () => {
    const confirmed = await ask(t("settings.meeting.recorder.stopConfirm"), {
      title: t("settings.meeting.recorder.stopTitle"),
      kind: "warning",
      okLabel: t("settings.meeting.recorder.stop"),
      cancelLabel: t("settings.meeting.recorder.keepRecording"),
    });
    if (confirmed) await stop();
  };

  if (phase === "idle") {
    return (
      <div className="flex flex-col items-center gap-3 py-4 text-center">
        <button
          type="button"
          onClick={() => void handleStart()}
          disabled={pendingAction !== null}
          aria-label={t("settings.meeting.recorder.start")}
          className="flex h-20 w-20 items-center justify-center rounded-full bg-red-600 text-white shadow-lg transition hover:scale-105 hover:bg-red-700 focus:outline-none focus-visible:ring-4 focus-visible:ring-red-500/40 disabled:cursor-not-allowed disabled:opacity-60 cursor-pointer"
        >
          {pendingAction === "start" ? (
            <Loader2 className="h-8 w-8 animate-spin" />
          ) : (
            <Mic className="h-9 w-9" />
          )}
        </button>
        <p className="text-base font-semibold">
          {t("settings.meeting.recorder.start")}
        </p>
        <p className="text-xs text-text/50">
          {t("settings.meeting.recorder.microphone", {
            name: settingsMicrophone,
          })}
        </p>
        <p className="max-w-md text-xs text-text/50">
          {t("settings.meeting.recorder.idleHint")}
        </p>
      </div>
    );
  }

  const microphone =
    status.microphone ?? t("settings.meeting.recorder.defaultMicrophone");

  return (
    <div
      className="flex flex-col items-center gap-3 py-2 text-center"
      role="status"
    >
      <div className="flex items-center gap-2 text-sm font-medium">
        {phase === "recording" && (
          <span className="relative flex h-3 w-3">
            <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-red-500 opacity-75" />
            <span className="relative inline-flex h-3 w-3 rounded-full bg-red-600" />
          </span>
        )}
        {phase === "paused" && <Pause className="h-4 w-4 text-yellow-500" />}
        {phase === "reconnecting" && (
          <Loader2 className="h-4 w-4 animate-spin text-yellow-500" />
        )}
        <span>{t(`settings.meeting.recorder.phase.${phase}`)}</span>
      </div>

      <p className="text-5xl font-semibold tabular-nums tracking-tight">
        {formatClock(status.durationMs)}
      </p>
      {current && (
        <p className="max-w-full truncate text-sm text-text/60">
          {current.name}
        </p>
      )}
      <LevelMeter level={phase === "recording" ? status.level : 0} />
      <p className="text-xs text-text/50">
        {t("settings.meeting.recorder.microphone", { name: microphone })}
        {" · "}
        {formatSize(status.sizeBytes)}
      </p>

      {phase === "reconnecting" && (
        <Alert variant="warning" className="text-left">
          {t("settings.meeting.recorder.reconnecting")}
        </Alert>
      )}

      <div className="flex flex-wrap items-center justify-center gap-3 pt-1">
        {phase === "paused" ? (
          <Button
            onClick={() => void handleResume()}
            variant="danger"
            size="lg"
            disabled={pendingAction !== null}
            className="flex items-center gap-2"
          >
            {pendingAction === "resume" ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Play className="h-4 w-4" fill="currentColor" />
            )}
            <span>{t("settings.meeting.recorder.resume")}</span>
          </Button>
        ) : (
          <Button
            onClick={() => void handlePause()}
            variant="secondary"
            size="lg"
            disabled={pendingAction !== null}
            className="flex items-center gap-2"
          >
            <Pause className="h-4 w-4" fill="currentColor" />
            <span>{t("settings.meeting.recorder.pause")}</span>
          </Button>
        )}
        <Button
          onClick={() => void handleStop()}
          variant="primary"
          size="lg"
          disabled={pendingAction !== null}
          className="flex items-center gap-2"
        >
          {pendingAction === "stop" ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <Square className="h-4 w-4" fill="currentColor" />
          )}
          <span>{t("settings.meeting.recorder.stop")}</span>
        </Button>
      </div>
      <p className="max-w-md text-xs text-text/40">
        {t("settings.meeting.recorder.backgroundHint")}
      </p>
    </div>
  );
};
