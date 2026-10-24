import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { writeHtml, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { documentDir } from "@tauri-apps/api/path";
import {
  Check,
  CircleDashed,
  Copy,
  FileText,
  Loader2,
  RotateCcw,
  Sparkles,
  X,
} from "lucide-react";
import { commands, type MeetingReportProgress } from "@/bindings";
import { useMeetingStore } from "@/stores/meetingStore";
import { useMeetingReportStore } from "@/stores/meetingReportStore";
import { markdownToHtml, formatGigabytes } from "@/lib/utils/summaryDocument";
import { Button } from "../ui/Button";
import { Alert } from "../ui/Alert";
import { ProgressBar } from "../shared";
import { MarkdownContent } from "../whats-new/MarkdownContent";
import { exportFileName, meetingErrorMessage } from "./format";
import { logoToPng } from "./logoPng";

/** Pourcentage d'une étape mesurable, `null` pour une attente sans mesure. */
const progressPercent = (progress: MeetingReportProgress): number | null => {
  switch (progress.phase) {
    case "transcribing":
      return progress.total > 0
        ? Math.round((progress.current / progress.total) * 100)
        : 0;
    case "engine":
    case "model":
      return progress.current;
    default:
      return null;
  }
};

// Ce que le compte rendu va appliquer : la personne voit d'avance si sa
// charte et son skill sont pris en compte.
const AppliedItem: React.FC<{ active: boolean; label: string }> = ({
  active,
  label,
}) => (
  <li className="flex items-center gap-2 text-sm">
    {active ? (
      <Check className="h-4 w-4 shrink-0 text-green-500" />
    ) : (
      <CircleDashed className="h-4 w-4 shrink-0 text-text/40" />
    )}
    <span className={active ? "" : "text-text/60"}>{label}</span>
  </li>
);

export const ReportPanel: React.FC = () => {
  const { t } = useTranslation();
  const { recordings, assets, status } = useMeetingStore();
  const {
    openId,
    reports,
    job,
    errors,
    engineInstalled,
    downloadBytes,
    close,
    generate,
    cancel,
  } = useMeetingReportStore();
  const [copied, setCopied] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [showTranscript, setShowTranscript] = useState(false);

  if (openId === null) return null;
  const recording = recordings.find((r) => r.id === openId);
  if (!recording) return null;

  const report = reports[openId];
  const markdown = report?.markdown ?? null;
  const transcript = report?.transcript ?? null;
  const error = errors[openId];
  const runningHere = job?.recordingId === openId;
  const runningElsewhere = job !== null && !runningHere;
  // Protège la capture : une réunion en cours d'enregistrement passe avant.
  const recorderBusy = status.phase !== "idle";
  const canGenerate = !job && !recorderBusy;

  const skillName =
    assets?.skill?.title ??
    assets?.skill?.fileName ??
    t("settings.meeting.report.applied.skillPasted");
  const charterName =
    assets?.editorial?.fileName ??
    t("settings.meeting.report.applied.charterPasted");

  const handleCopy = async () => {
    if (!markdown) return;
    try {
      await writeHtml(markdownToHtml(markdown), markdown);
    } catch {
      await writeText(markdown);
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  // Document Word aux couleurs de la charte, par défaut dans Documents.
  const handleExport = async () => {
    if (!markdown) return;
    const fileName = exportFileName(recording.name).replace(/\.wav$/, ".docx");
    let defaultPath = fileName;
    try {
      defaultPath = `${await documentDir()}/${fileName}`;
    } catch (e) {
      console.error("Failed to resolve documents directory:", e);
    }
    const target = await save({
      defaultPath,
      filters: [{ name: "Word", extensions: ["docx"] }],
    });
    if (!target) return;
    setExporting(true);
    try {
      let logo = null;
      if (assets?.logoPath) {
        try {
          logo = await logoToPng(assets.logoPath);
        } catch (e) {
          console.error("Failed to convert logo:", e);
          toast.warning(t("settings.meeting.report.logoSkipped"));
        }
      }
      const result = await commands.meetingExportReportDocx(
        openId,
        target,
        logo,
      );
      if (result.status === "error") {
        toast.error(meetingErrorMessage(t, result.error));
        return;
      }
      toast.success(t("settings.meeting.report.exported"));
      revealItemInDir(target).catch(() => {});
    } finally {
      setExporting(false);
    }
  };

  const progress = runningHere ? job.progress : null;
  const percent = progress ? progressPercent(progress) : null;
  const phaseLabel = progress
    ? t(`settings.meeting.report.phase.${progress.phase}`, {
        current: progress.current,
        total: progress.total,
        percent,
      })
    : t("settings.meeting.report.phase.preparing");

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between gap-2 px-4">
        <div className="min-w-0">
          <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
            {t("settings.meeting.report.title")}
          </h2>
          <p className="truncate text-sm text-text/70" title={recording.name}>
            {recording.name}
          </p>
        </div>
        <button
          type="button"
          onClick={close}
          title={t("settings.meeting.report.close")}
          className="rounded p-1.5 text-text/40 transition-colors hover:bg-mid-gray/10 hover:text-text"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      <div className="bg-background border border-mid-gray/20 rounded-lg p-4 space-y-4">
        {runningHere && (
          <div
            role="status"
            className="space-y-2 rounded-lg border border-logo-primary/30 bg-logo-primary/5 p-4"
          >
            <div className="flex items-center justify-between gap-2">
              <div className="flex items-center gap-2 text-sm">
                <Loader2 className="h-4 w-4 animate-spin text-logo-primary" />
                <span>{phaseLabel}</span>
              </div>
              <Button
                onClick={() => void cancel()}
                variant="secondary"
                size="sm"
                disabled={job.cancelling}
                className="flex items-center gap-2"
              >
                <X className="h-4 w-4" />
                <span>{t("settings.meeting.report.cancel")}</span>
              </Button>
            </div>
            {percent !== null && progress && (
              <ProgressBar
                size="full"
                progress={[{ id: progress.phase, percentage: percent }]}
              />
            )}
            <p className="text-xs text-text/50">
              {t("settings.meeting.report.runningHint")}
            </p>
          </div>
        )}

        {runningElsewhere && (
          <Alert variant="info">
            {t("settings.meeting.report.busyElsewhere")}
          </Alert>
        )}
        {error && !runningHere && (
          <Alert variant="error">{meetingErrorMessage(t, error)}</Alert>
        )}

        {!markdown && !runningHere && (
          <div className="space-y-3">
            <p className="text-sm text-text/70">
              {t("settings.meeting.report.intro")}
            </p>
            <ul className="space-y-1.5">
              <AppliedItem
                active={assets?.editorial != null}
                label={
                  assets?.editorial
                    ? t("settings.meeting.report.applied.charter", {
                        name: charterName,
                      })
                    : t("settings.meeting.report.applied.noCharter")
                }
              />
              <AppliedItem
                active={assets?.skill != null}
                label={
                  assets?.skill
                    ? t("settings.meeting.report.applied.skill", {
                        name: skillName,
                      })
                    : t("settings.meeting.report.applied.defaultSkill")
                }
              />
              <AppliedItem
                active={assets?.graphic != null || assets?.logoPath != null}
                label={
                  assets?.graphic != null || assets?.logoPath != null
                    ? t("settings.meeting.report.applied.graphic")
                    : t("settings.meeting.report.applied.noGraphic")
                }
              />
            </ul>
            {engineInstalled === false && (
              <p className="text-xs text-text/50">
                {downloadBytes > 0
                  ? t("settings.meeting.report.firstUse", {
                      size: formatGigabytes(downloadBytes),
                    })
                  : t("settings.meeting.report.firstUseNoSize")}
              </p>
            )}
            {recorderBusy && (
              <p className="text-xs text-yellow-600">
                {t("settings.meeting.report.recorderBusy")}
              </p>
            )}
            <Button
              onClick={() => void generate(openId)}
              variant="primary"
              size="md"
              disabled={!canGenerate}
              className="flex items-center gap-2"
            >
              <Sparkles className="h-4 w-4" />
              <span>{t("settings.meeting.report.generate")}</span>
            </Button>
          </div>
        )}

        {markdown && (
          <div className="space-y-3">
            <div className="flex flex-wrap items-center gap-2">
              <Button
                onClick={() => void handleExport()}
                variant="primary"
                size="sm"
                disabled={exporting}
                className="flex items-center gap-2"
              >
                {exporting ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <FileText className="h-4 w-4" />
                )}
                <span>{t("settings.meeting.report.exportWord")}</span>
              </Button>
              <Button
                onClick={() => void handleCopy()}
                variant="secondary"
                size="sm"
                className="flex items-center gap-2"
              >
                {copied ? (
                  <Check className="h-4 w-4" />
                ) : (
                  <Copy className="h-4 w-4" />
                )}
                <span>
                  {copied
                    ? t("settings.meeting.report.copied")
                    : t("settings.meeting.report.copy")}
                </span>
              </Button>
              <Button
                onClick={() => void generate(openId)}
                variant="ghost"
                size="sm"
                disabled={!canGenerate}
                className="flex items-center gap-2"
              >
                <RotateCcw className="h-4 w-4" />
                <span>{t("settings.meeting.report.regenerate")}</span>
              </Button>
            </div>
            <Alert variant="info">
              {t("settings.meeting.report.draftNotice")}
            </Alert>
            <div className="border border-logo-primary/30 rounded-lg p-4 max-h-[60vh] overflow-y-auto select-text">
              <MarkdownContent markdown={markdown} />
            </div>
          </div>
        )}

        {transcript && (
          <div className="space-y-2">
            <button
              type="button"
              onClick={() => setShowTranscript((v) => !v)}
              className="text-xs text-text/60 underline-offset-2 hover:underline"
            >
              {showTranscript
                ? t("settings.meeting.report.hideTranscript")
                : t("settings.meeting.report.showTranscript")}
            </button>
            {showTranscript && (
              <div className="max-h-[40vh] overflow-y-auto whitespace-pre-wrap rounded-lg border border-mid-gray/20 bg-mid-gray/5 p-3 text-sm text-text/80 select-text">
                {transcript}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
};
