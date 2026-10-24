import React, { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ask, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { convertFileSrc } from "@tauri-apps/api/core";
import { downloadDir } from "@tauri-apps/api/path";
import {
  Check,
  Download,
  FileText,
  Loader2,
  Mic,
  Pencil,
  Play,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { commands, type MeetingRecording } from "@/bindings";
import { useMeetingStore } from "@/stores/meetingStore";
import { useMeetingReportStore } from "@/stores/meetingReportStore";
import { AudioPlayer } from "../ui/AudioPlayer";
import {
  exportFileName,
  formatClock,
  formatDuration,
  formatSize,
  meetingErrorMessage,
  sortRecordings,
  type RecordingSort,
} from "./format";

const SORTS: RecordingSort[] = ["newest", "oldest", "name", "longest"];

const iconButton =
  "p-1.5 rounded shrink-0 text-text/40 transition-colors hover:text-logo-primary hover:bg-logo-primary/10 disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:bg-transparent disabled:hover:text-text/40";

export const RecordingList: React.FC = () => {
  const { t, i18n } = useTranslation();
  const { recordings, status, highlightedId, rename, remove } =
    useMeetingStore();
  const openReportId = useMeetingReportStore((s) => s.openId);
  const reportJobId = useMeetingReportStore((s) => s.job?.recordingId ?? null);
  const openReport = useMeetingReportStore((s) => s.open);
  const forgetReport = useMeetingReportStore((s) => s.forget);
  const [sort, setSort] = useState<RecordingSort>("newest");
  const [query, setQuery] = useState("");
  const [editingId, setEditingId] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  // Garde-fou contre une double validation (Entrée puis `blur` du champ qui
  // disparaît) : seul le premier appel renomme.
  const editingRef = useRef<number | null>(null);
  const [exportingId, setExportingId] = useState<number | null>(null);
  const [player, setPlayer] = useState<{ id: number; src: string } | null>(
    null,
  );

  const visible = useMemo(
    () => sortRecordings(recordings, sort, query),
    [recordings, sort, query],
  );

  const formatDate = (unixSeconds: number): string =>
    new Date(unixSeconds * 1000).toLocaleString(i18n.language, {
      dateStyle: "short",
      timeStyle: "short",
    });

  const startEditing = (recording: MeetingRecording) => {
    editingRef.current = recording.id;
    setEditingId(recording.id);
    setDraft(recording.name);
  };

  const cancelEditing = () => {
    editingRef.current = null;
    setEditingId(null);
  };

  const commitRename = async (recording: MeetingRecording) => {
    if (editingRef.current !== recording.id) return;
    editingRef.current = null;
    const name = draft.trim();
    setEditingId(null);
    if (!name || name === recording.name) return;
    const error = await rename(recording.id, name);
    if (error) toast.error(meetingErrorMessage(t, error));
  };

  const handlePlay = async (recording: MeetingRecording) => {
    if (player?.id === recording.id) {
      setPlayer(null);
      return;
    }
    const result = await commands.meetingAudioPath(recording.id);
    if (result.status === "error") {
      toast.error(meetingErrorMessage(t, result.error));
      return;
    }
    setPlayer({ id: recording.id, src: convertFileSrc(result.data) });
  };

  // Copie du WAV là où l'utilisateur le choisit, par défaut dans
  // Téléchargements, puis ouverture du dossier.
  const handleExport = async (recording: MeetingRecording) => {
    const fileName = exportFileName(recording.name);
    let defaultPath = fileName;
    try {
      defaultPath = `${await downloadDir()}/${fileName}`;
    } catch (error) {
      console.error("Failed to resolve downloads directory:", error);
    }
    const target = await save({
      defaultPath,
      filters: [{ name: "WAV", extensions: ["wav"] }],
    });
    if (!target) return;
    setExportingId(recording.id);
    try {
      const result = await commands.meetingExport(recording.id, target);
      if (result.status === "error") {
        toast.error(meetingErrorMessage(t, result.error));
        return;
      }
      toast.success(t("settings.meeting.list.exported"));
      revealItemInDir(target).catch(() => {});
    } finally {
      setExportingId(null);
    }
  };

  const handleDelete = async (recording: MeetingRecording) => {
    const confirmed = await ask(
      t("settings.meeting.list.deleteConfirm", { name: recording.name }),
      { title: t("settings.meeting.list.deleteTitle"), kind: "warning" },
    );
    if (!confirmed) return;
    if (player?.id === recording.id) setPlayer(null);
    const error = await remove(recording.id);
    if (error) {
      toast.error(meetingErrorMessage(t, error));
    } else {
      forgetReport(recording.id);
      toast.success(t("settings.meeting.list.deleted"));
    }
  };

  // Le panneau du compte rendu s'ouvre au-dessus de la liste : on y remonte.
  const handleOpenReport = (recording: MeetingRecording) => {
    void openReport(recording.id);
    document
      .querySelector(".overflow-y-auto")
      ?.scrollTo({ top: 0, behavior: "smooth" });
  };

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center justify-between gap-2 px-4">
        <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
          {t("settings.meeting.list.title")}
        </h2>
        {recordings.length > 1 && (
          <div className="flex items-center gap-2">
            <div className="relative">
              <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-text/40" />
              <input
                type="search"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder={t("settings.meeting.list.search")}
                aria-label={t("settings.meeting.list.search")}
                className="w-40 rounded-md border border-mid-gray/40 bg-mid-gray/10 py-1 pl-7 pr-2 text-xs placeholder:text-text/40 focus:outline-none focus:ring-1 focus:ring-logo-primary"
              />
            </div>
            <select
              value={sort}
              onChange={(e) => setSort(e.target.value as RecordingSort)}
              aria-label={t("settings.meeting.list.sortLabel")}
              className="rounded-md border border-mid-gray/40 bg-mid-gray/10 px-2 py-1 text-xs focus:outline-none focus:ring-1 focus:ring-logo-primary cursor-pointer"
            >
              {SORTS.map((key) => (
                <option key={key} value={key}>
                  {t(`settings.meeting.list.sort.${key}`)}
                </option>
              ))}
            </select>
          </div>
        )}
      </div>

      <div className="bg-background border border-mid-gray/20 rounded-lg divide-y divide-mid-gray/10">
        {recordings.length === 0 && (
          <p className="px-4 py-6 text-center text-sm text-text/50">
            {t("settings.meeting.list.empty")}
          </p>
        )}
        {recordings.length > 0 && visible.length === 0 && (
          <p className="px-4 py-6 text-center text-sm text-text/50">
            {t("settings.meeting.list.noMatch")}
          </p>
        )}
        {visible.map((recording) => {
          const inProgress = recording.state === "inProgress";
          const isCurrent = inProgress && recording.id === status.recordingId;
          const durationLabel = isCurrent
            ? formatClock(status.durationMs)
            : formatDuration(recording.durationMs);
          const sizeLabel = formatSize(
            isCurrent ? status.sizeBytes : recording.sizeBytes,
          );
          const editing = editingId === recording.id;

          return (
            <div
              key={recording.id}
              className={`px-4 py-2.5 space-y-2 transition-colors ${
                recording.id === highlightedId ? "bg-logo-primary/5" : ""
              }`}
            >
              <div className="flex items-center gap-3">
                <Mic
                  className={`w-4 h-4 shrink-0 ${inProgress ? "text-red-500" : "text-text/40"}`}
                />
                <div className="flex-1 min-w-0">
                  {editing ? (
                    <div className="flex items-center gap-1">
                      <input
                        autoFocus
                        value={draft}
                        maxLength={200}
                        onChange={(e) => setDraft(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") void commitRename(recording);
                          if (e.key === "Escape") cancelEditing();
                        }}
                        onBlur={() => void commitRename(recording)}
                        aria-label={t("settings.meeting.list.rename")}
                        className="flex-1 min-w-0 rounded-md border border-logo-primary bg-mid-gray/10 px-2 py-0.5 text-sm font-medium focus:outline-none select-text"
                      />
                      <button
                        type="button"
                        // Avant le `blur` du champ, qui validerait le nom.
                        onMouseDown={(e) => e.preventDefault()}
                        onClick={() => void commitRename(recording)}
                        title={t("settings.meeting.list.renameSave")}
                        className={iconButton}
                      >
                        <Check className="w-4 h-4" />
                      </button>
                      <button
                        type="button"
                        onMouseDown={(e) => e.preventDefault()}
                        onClick={cancelEditing}
                        title={t("settings.meeting.list.renameCancel")}
                        className={iconButton}
                      >
                        <X className="w-4 h-4" />
                      </button>
                    </div>
                  ) : (
                    <p
                      className="text-sm font-medium text-text truncate"
                      title={recording.name}
                    >
                      {recording.name}
                    </p>
                  )}
                  <p className="flex flex-wrap items-center gap-x-1.5 text-xs text-text/50">
                    <span>{formatDate(recording.createdAt)}</span>
                    <span aria-hidden="true">·</span>
                    <span className="tabular-nums">{durationLabel}</span>
                    <span aria-hidden="true">·</span>
                    <span>{sizeLabel}</span>
                    {inProgress && (
                      <span className="rounded bg-red-500/15 px-1.5 py-0.5 text-[10px] font-medium uppercase text-red-500">
                        {t("settings.meeting.list.inProgress")}
                      </span>
                    )}
                    {recording.state === "interrupted" && (
                      <span
                        className="rounded bg-yellow-500/15 px-1.5 py-0.5 text-[10px] font-medium uppercase text-yellow-600"
                        title={t("settings.meeting.list.interruptedHint")}
                      >
                        {t("settings.meeting.list.interrupted")}
                      </span>
                    )}
                  </p>
                </div>

                {!editing && (
                  <div className="flex items-center gap-0.5">
                    {!inProgress && (
                      <button
                        type="button"
                        onClick={() => handleOpenReport(recording)}
                        title={t("settings.meeting.list.reportHint")}
                        className={`mr-1 flex items-center gap-1.5 rounded-md px-2 py-1 text-xs font-medium transition-colors ${
                          openReportId === recording.id
                            ? "bg-logo-primary/30 text-text"
                            : "bg-logo-primary/15 text-text hover:bg-logo-primary/25"
                        }`}
                      >
                        {reportJobId === recording.id ? (
                          <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        ) : recording.hasReport ? (
                          <Check className="h-3.5 w-3.5 text-green-500" />
                        ) : (
                          <FileText className="h-3.5 w-3.5" />
                        )}
                        <span>{t("settings.meeting.list.report")}</span>
                      </button>
                    )}
                    <button
                      type="button"
                      onClick={() => void handlePlay(recording)}
                      disabled={inProgress}
                      title={t("settings.meeting.list.play")}
                      className={`${iconButton} ${player?.id === recording.id ? "text-logo-primary" : ""}`}
                    >
                      <Play className="w-4 h-4" />
                    </button>
                    <button
                      type="button"
                      onClick={() => startEditing(recording)}
                      title={t("settings.meeting.list.rename")}
                      className={iconButton}
                    >
                      <Pencil className="w-4 h-4" />
                    </button>
                    <button
                      type="button"
                      onClick={() => void handleExport(recording)}
                      disabled={inProgress || exportingId !== null}
                      title={t("settings.meeting.list.export")}
                      className={iconButton}
                    >
                      {exportingId === recording.id ? (
                        <Loader2 className="w-4 h-4 animate-spin" />
                      ) : (
                        <Download className="w-4 h-4" />
                      )}
                    </button>
                    <button
                      type="button"
                      onClick={() => void handleDelete(recording)}
                      // Le compte rendu en cours lit ce fichier.
                      disabled={inProgress || reportJobId === recording.id}
                      title={t("settings.meeting.list.delete")}
                      className={`${iconButton} hover:!text-red-500 hover:!bg-red-500/10`}
                    >
                      <Trash2 className="w-4 h-4" />
                    </button>
                  </div>
                )}
              </div>

              {player?.id === recording.id && (
                <AudioPlayer
                  key={player.src}
                  src={player.src}
                  autoPlay
                  className="pl-7"
                />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
};
