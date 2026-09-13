import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ask, open, save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { writeHtml, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { downloadDir } from "@tauri-apps/api/path";
import {
  Check,
  Copy,
  Download,
  FileAudio,
  Film,
  FolderOpen,
  Link2,
  Loader2,
  RotateCcw,
  Save,
  Sparkles,
  Trash2,
  UploadCloud,
  X,
} from "lucide-react";
import type { SummaryError } from "@/bindings";
import {
  SUMMARY_ACTIVE_STATUSES,
  useFileTranscriptionStore,
} from "@/stores/fileTranscriptionStore";
import {
  buildExportMarkdown,
  formatGigabytes,
  markdownToHtml,
} from "@/lib/utils/summaryDocument";
import { Button } from "../ui/Button";
import { Alert } from "../ui/Alert";
import { ProgressBar } from "../shared";
import { MarkdownContent } from "../whats-new/MarkdownContent";

// Formats décodés nativement (symphonia) ou via le repli ffmpeg (issue #10).
const ACCEPTED_EXTENSIONS = [
  "mp3",
  "mp4",
  "m4a",
  "mov",
  "wav",
  "aac",
  "flac",
  "ogg",
  "oga",
  "opus",
  "aiff",
  "aif",
  "caf",
  "mkv",
  "webm",
  "3gp",
  "amr",
  "wma",
  "wmv",
  "avi",
];

const getFileName = (path: string): string => {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
};

const getExtension = (path: string): string => {
  const name = getFileName(path);
  const dotIndex = name.lastIndexOf(".");
  return dotIndex === -1 ? "" : name.slice(dotIndex + 1).toLowerCase();
};

// L'état de la transcription vit dans `useFileTranscriptionStore` (pas ici) :
// il survit aux changements d'onglet, et l'écouteur de progression du store
// suit une transcription en cours même quand cette page est démontée. Seul
// l'éphémère purement visuel (survol de drag, "copié !", enregistrement en
// cours) reste en état local.
export const FileTranscription: React.FC = () => {
  const { t, i18n } = useTranslation();
  const {
    status,
    sourceLabel,
    sourceKind,
    videoDownloads,
    progress,
    outputPath,
    markdown,
    errorMessage,
    historyId,
    cancelling,
    urlInput,
    history,
    summaryStatus,
    summaryProgress,
    summaryNeedsDownload,
    summaryInstalled,
    summaryDownloadBytes,
    summaryMarkdown,
    summaryError,
    cancellingSummary,
    initialize,
    setUrlInput,
    startFile,
    startUrl,
    cancel,
    reset,
    openHistoryEntry,
    deleteHistoryEntry,
    downloadVideo,
    exportVideo,
    summarize,
    cancelSummary,
  } = useFileTranscriptionStore();

  const [isDragOver, setIsDragOver] = useState(false);
  const [copiedWhich, setCopiedWhich] = useState<"raw" | "summary" | null>(
    null,
  );
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    void initialize();
  }, [initialize]);

  const isProcessing = status === "processing";
  const summaryRunning = SUMMARY_ACTIVE_STATUSES.includes(summaryStatus);
  // Une transcription ou un résumé en cours occupe le moteur : pas de
  // nouveau fichier ni de changement d'entrée pendant ce temps.
  const busy = isProcessing || summaryRunning;

  const startTranscription = useCallback(
    async (path: string) => {
      const extension = getExtension(path);
      if (!ACCEPTED_EXTENSIONS.includes(extension)) {
        toast.error(t("settings.file.errorTitle"), {
          description: t("settings.file.dropzoneHint"),
        });
        return;
      }
      await startFile(path);
    },
    [startFile, t],
  );

  // Drag & drop: Tauri intercepts HTML5 DnD at the webview level and gives us
  // real filesystem paths instead (unlike `event.dataTransfer.files`, which
  // Tauri leaves empty for security reasons).
  useEffect(() => {
    const unlistenPromise = getCurrentWebview().onDragDropEvent((event) => {
      const state = useFileTranscriptionStore.getState();
      if (
        state.status === "processing" ||
        SUMMARY_ACTIVE_STATUSES.includes(state.summaryStatus)
      ) {
        return;
      }

      if (event.payload.type === "enter" || event.payload.type === "over") {
        setIsDragOver(true);
      } else if (event.payload.type === "leave") {
        setIsDragOver(false);
      } else if (event.payload.type === "drop") {
        setIsDragOver(false);
        const [firstPath] = event.payload.paths;
        if (firstPath) {
          void startTranscription(firstPath);
        }
      }
    });

    return () => {
      unlistenPromise.then((fn) => fn());
    };
  }, [startTranscription]);

  const handleBrowse = async () => {
    try {
      const selected = await open({
        multiple: false,
        filters: [{ name: "Audio/Vidéo", extensions: ACCEPTED_EXTENSIONS }],
      });
      if (typeof selected === "string") {
        await startTranscription(selected);
      }
    } catch (error) {
      console.error("Failed to open file picker:", error);
      toast.error(t("settings.file.browseError"));
    }
  };

  const handleUrlSubmit = async () => {
    const url = urlInput.trim();
    if (busy || !url) return;
    if (!/^https?:\/\/.+/i.test(url)) {
      toast.error(t("settings.file.url.invalid"));
      return;
    }
    await startUrl(url);
  };

  const flashCopied = (which: "raw" | "summary") => {
    setCopiedWhich(which);
    setTimeout(() => setCopiedWhich(null), 2000);
  };

  const handleCopy = async (text: string) => {
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      flashCopied("raw");
    } catch (error) {
      console.error("Failed to copy transcription document:", error);
    }
  };

  // Compte-rendu en texte enrichi (Word, Outlook, Notes…) avec repli texte
  // brut : le Markdown reste lisible dans un éditeur simple.
  const handleCopySummary = async () => {
    if (!summaryMarkdown) return;
    try {
      await writeHtml(markdownToHtml(summaryMarkdown), summaryMarkdown);
    } catch (error) {
      console.error(
        "Failed to copy summary as HTML, plain text fallback:",
        error,
      );
      try {
        await writeText(summaryMarkdown);
      } catch (fallbackError) {
        console.error("Failed to copy summary:", fallbackError);
        toast.error(t("settings.file.summary.copyError"));
        return;
      }
    }
    flashCopied("summary");
  };

  const handleSaveAs = async () => {
    if (!markdown) return;
    setSaving(true);
    try {
      const target = await save({
        defaultPath: outputPath ?? "transcription.md",
        filters: [{ name: "Markdown", extensions: ["md"] }],
      });
      if (target) {
        await writeTextFile(
          target,
          buildExportMarkdown(markdown, summaryMarkdown),
        );
        toast.success(t("settings.file.saveSuccess"));
      }
    } catch (error) {
      console.error("Failed to save transcription document:", error);
      toast.error(t("settings.file.saveError"));
    } finally {
      setSaving(false);
    }
  };

  const handleOpenFolder = async () => {
    if (!outputPath) return;
    try {
      await revealItemInDir(outputPath);
    } catch (error) {
      console.error("Failed to reveal transcription document:", error);
      toast.error(t("settings.file.openFolderError"));
    }
  };

  // Télécharge la vidéo (au premier clic, sinon réutilise la copie conservée)
  // puis propose de l'enregistrer — par défaut dans le dossier Téléchargements.
  const handleSaveVideo = async (id: number, name: string) => {
    if (videoDownloads[id] !== undefined) return;

    const downloaded = await downloadVideo(id);
    if (!downloaded.ok) {
      if (downloaded.error !== "already downloading") {
        toast.error(t("settings.file.video.error"), {
          description: downloaded.error,
        });
      }
      return;
    }

    let defaultPath = `${name}.mp4`;
    try {
      defaultPath = `${await downloadDir()}/${name}.mp4`;
    } catch (error) {
      console.error("Failed to resolve downloads directory:", error);
    }
    const target = await save({
      defaultPath,
      filters: [{ name: "MP4", extensions: ["mp4"] }],
    });
    if (!target) return;

    const exported = await exportVideo(id, target);
    if (exported) {
      toast.success(t("settings.file.video.saved"));
      revealItemInDir(target).catch(() => {});
    } else {
      toast.error(t("settings.file.video.error"));
    }
  };

  const handleOpenHistoryEntry = async (id: number) => {
    const opened = await openHistoryEntry(id);
    if (!opened) {
      toast.error(t("settings.file.history.openError"));
    }
  };

  const handleDeleteHistoryEntry = async (id: number, name: string) => {
    const confirmed = await ask(
      t("settings.file.history.deleteConfirm", { name }),
      {
        title: t("settings.file.history.deleteTitle"),
        kind: "warning",
      },
    );
    if (!confirmed) return;
    const deleted = await deleteHistoryEntry(id);
    if (deleted) {
      toast.success(t("settings.file.history.deleted"));
    }
  };

  const formatDate = (unixSeconds: number): string =>
    new Date(unixSeconds * 1000).toLocaleString(i18n.language, {
      dateStyle: "short",
      timeStyle: "short",
    });

  const phaseLabel = progress
    ? t(`settings.file.phase.${progress.phase}`, {
        current: progress.current,
        total: progress.total,
      })
    : null;

  // Taille annoncée pour le téléchargement unique (moteur + modèle), ou `null`
  // quand le backend n'annonce rien à télécharger (0 octet : aucun asset pour
  // cette plateforme). On bascule alors sur les variantes sans taille des
  // messages, plutôt que d'annoncer « 0 Go » ou une taille inventée.
  const downloadSizeLabel =
    summaryDownloadBytes > 0 ? formatGigabytes(summaryDownloadBytes) : null;

  // Chaque variante de `SummaryError` a son message (spec plan 09, §5) ;
  // `cancelled` n'en a pas (retour silencieux).
  const describeSummaryError = (error: SummaryError): string => {
    switch (error.kind) {
      case "offline":
        // Même taille que l'encart de progression : une seule vérité.
        return downloadSizeLabel
          ? t("settings.file.summary.errors.offline", {
              size: downloadSizeLabel,
            })
          : t("settings.file.summary.errors.offlineNoSize");
      case "diskSpace":
        return t("settings.file.summary.errors.diskSpace", {
          free: formatGigabytes(error.freeBytes),
        });
      case "memory":
        return t("settings.file.summary.errors.memory");
      case "downloadFailed":
        return t("settings.file.summary.errors.downloadFailed");
      case "checksumMismatch":
        return t("settings.file.summary.errors.checksumMismatch");
      case "engineStartFailed":
        return t("settings.file.summary.errors.engineStartFailed");
      case "incompleteOutput":
        return t("settings.file.summary.errors.incompleteOutput");
      case "busy":
        return t("settings.file.summary.errors.busy");
      case "cancelled":
        return "";
    }
  };

  // Annonce du téléchargement unique, avant le premier résumé puis dans
  // l'encart de progression.
  const firstUseHint = downloadSizeLabel
    ? t("settings.file.summary.firstUseHint", { size: downloadSizeLabel })
    : t("settings.file.summary.firstUseHintNoSize");
  const preparingLabel = !summaryNeedsDownload
    ? t("settings.file.summary.preparing")
    : downloadSizeLabel
      ? t("settings.file.summary.preparingDownload", {
          size: downloadSizeLabel,
        })
      : t("settings.file.summary.preparingDownloadNoSize");

  // Titre de l'encart : téléchargement unique annoncé, puis phase courante.
  // La phase `summarizing` est un décompte de parties (« partie n/m ») dont le
  // total peut grandir en cours de route : pas de pourcentage, pas de barre.
  const summaryTitle = summaryProgress
    ? t(`settings.file.summary.phase.${summaryProgress.phase}`, {
        current: summaryProgress.current,
        total: summaryProgress.total,
      })
    : preparingLabel;
  const summaryDownloading =
    summaryProgress !== null &&
    (summaryProgress.phase === "engine" || summaryProgress.phase === "model");

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <div className="space-y-2">
        <div className="px-4">
          <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
            {t("settings.file.title")}
          </h2>
          <p className="text-sm text-text/60 mt-1">
            {t("settings.file.description")}
          </p>
        </div>

        <div className="bg-background border border-mid-gray/20 rounded-lg p-4 space-y-4">
          {status !== "done" && (
            <>
              <div
                className={`flex flex-col items-center justify-center gap-3 rounded-lg border-2 border-dashed p-8 text-center transition-colors ${
                  isDragOver
                    ? "border-logo-primary bg-logo-primary/10"
                    : "border-mid-gray/30"
                }`}
              >
                {isProcessing ? (
                  <Loader2 className="w-8 h-8 animate-spin text-logo-primary" />
                ) : (
                  <UploadCloud className="w-8 h-8 text-text/40" />
                )}

                <div>
                  <p className="text-sm font-medium text-text break-all">
                    {isProcessing
                      ? (sourceLabel ?? "")
                      : t("settings.file.dropzoneTitle")}
                  </p>
                  {!isProcessing && (
                    <p className="text-xs text-text/50 mt-1">
                      {t("settings.file.dropzoneHint")}
                    </p>
                  )}
                  {isProcessing && phaseLabel && (
                    <p className="text-xs text-text/60 mt-1">{phaseLabel}</p>
                  )}
                </div>

                {isProcessing ? (
                  <Button
                    onClick={() => void cancel()}
                    variant="secondary"
                    size="sm"
                    disabled={cancelling}
                    className="flex items-center gap-2"
                  >
                    <X className="w-4 h-4" />
                    <span>{t("settings.file.cancel")}</span>
                  </Button>
                ) : (
                  <Button
                    onClick={handleBrowse}
                    variant="primary-soft"
                    size="sm"
                    className="flex items-center gap-2"
                  >
                    <FileAudio className="w-4 h-4" />
                    <span>{t("settings.file.browse")}</span>
                  </Button>
                )}
              </div>

              {/* Transcription directe depuis une vidéo en ligne (yt-dlp). */}
              {!isProcessing && (
                <div className="space-y-2">
                  <p className="text-xs text-text/50">
                    {t("settings.file.url.label")}
                  </p>
                  <div className="flex items-center gap-2">
                    <div className="relative flex-1">
                      <Link2 className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-text/40 pointer-events-none" />
                      <input
                        type="text"
                        value={urlInput}
                        onChange={(e) => setUrlInput(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") void handleUrlSubmit();
                        }}
                        placeholder={t("settings.file.url.placeholder")}
                        className="w-full pl-9 pr-3 py-2 text-sm bg-mid-gray/10 border border-mid-gray/40 rounded-lg focus:outline-none focus:ring-1 focus:ring-logo-primary placeholder:text-text/40"
                      />
                    </div>
                    <Button
                      onClick={() => void handleUrlSubmit()}
                      variant="primary-soft"
                      size="sm"
                      disabled={!urlInput.trim()}
                      className="flex items-center gap-2"
                    >
                      <Download className="w-4 h-4" />
                      <span>{t("settings.file.url.button")}</span>
                    </Button>
                  </div>
                </div>
              )}
            </>
          )}

          {status === "error" && errorMessage && (
            <Alert variant="error">{errorMessage}</Alert>
          )}

          {status === "done" && markdown && (
            <div className="space-y-4">
              <div className="flex items-center justify-between gap-2 flex-wrap">
                <p className="text-xs text-text/50 truncate">
                  {sourceLabel &&
                    t("settings.file.selectedFile", {
                      name: sourceLabel,
                    })}
                </p>
                <div className="flex items-center gap-2 flex-wrap">
                  <Button
                    onClick={() => void handleCopy(markdown)}
                    variant="secondary"
                    size="sm"
                    className="flex items-center gap-2"
                  >
                    {copiedWhich === "raw" ? (
                      <Check className="w-4 h-4" />
                    ) : (
                      <Copy className="w-4 h-4" />
                    )}
                    <span>
                      {copiedWhich === "raw"
                        ? t("settings.file.copied")
                        : t("settings.file.copyRaw")}
                    </span>
                  </Button>
                  <Button
                    onClick={() => void summarize()}
                    variant="primary-soft"
                    size="sm"
                    disabled={busy}
                    title={
                      summaryInstalled === false ? firstUseHint : undefined
                    }
                    className="flex items-center gap-2"
                  >
                    {summaryRunning ? (
                      <Loader2 className="w-4 h-4 animate-spin" />
                    ) : (
                      <Sparkles className="w-4 h-4" />
                    )}
                    <span>{t("settings.file.summary.button")}</span>
                  </Button>
                  {summaryMarkdown !== null && (
                    <Button
                      onClick={() => void handleCopySummary()}
                      variant="secondary"
                      size="sm"
                      className="flex items-center gap-2"
                    >
                      {copiedWhich === "summary" ? (
                        <Check className="w-4 h-4" />
                      ) : (
                        <Copy className="w-4 h-4" />
                      )}
                      <span>
                        {copiedWhich === "summary"
                          ? t("settings.file.copied")
                          : t("settings.file.summary.copy")}
                      </span>
                    </Button>
                  )}
                  <Button
                    onClick={handleSaveAs}
                    variant="secondary"
                    size="sm"
                    disabled={saving}
                    className="flex items-center gap-2"
                  >
                    <Save className="w-4 h-4" />
                    <span>{t("settings.file.saveAs")}</span>
                  </Button>
                  {outputPath !== null && (
                    <Button
                      onClick={handleOpenFolder}
                      variant="secondary"
                      size="sm"
                      className="flex items-center gap-2"
                    >
                      <FolderOpen className="w-4 h-4" />
                      <span>{t("settings.file.openFolder")}</span>
                    </Button>
                  )}
                  {sourceKind === "url" && historyId !== null && (
                    <Button
                      onClick={() =>
                        void handleSaveVideo(historyId, sourceLabel ?? "video")
                      }
                      variant="secondary"
                      size="sm"
                      disabled={videoDownloads[historyId] !== undefined}
                      className="flex items-center gap-2"
                    >
                      {videoDownloads[historyId] !== undefined ? (
                        <Loader2 className="w-4 h-4 animate-spin" />
                      ) : (
                        <Film className="w-4 h-4" />
                      )}
                      <span>
                        {videoDownloads[historyId] !== undefined
                          ? t("settings.file.video.downloading", {
                              percent: videoDownloads[historyId],
                            })
                          : t("settings.file.video.save")}
                      </span>
                    </Button>
                  )}
                  <Button
                    onClick={reset}
                    variant="ghost"
                    size="sm"
                    disabled={busy}
                    className="flex items-center gap-2"
                  >
                    <RotateCcw className="w-4 h-4" />
                    <span>{t("settings.file.newFile")}</span>
                  </Button>
                </div>
              </div>

              {summaryInstalled === false && !summaryRunning && (
                <p className="text-xs text-text/50">{firstUseHint}</p>
              )}

              {/* Encart de progression du compte-rendu (spec plan 09, §5). */}
              {summaryRunning && (
                <div className="space-y-2 rounded-lg border border-logo-primary/30 bg-logo-primary/5 p-4">
                  <div className="flex items-center justify-between gap-2">
                    <div className="flex items-center gap-2 text-sm text-text">
                      <Loader2 className="w-4 h-4 animate-spin text-logo-primary" />
                      <span>{summaryTitle}</span>
                    </div>
                    <Button
                      onClick={() => void cancelSummary()}
                      variant="secondary"
                      size="sm"
                      disabled={cancellingSummary}
                      className="flex items-center gap-2"
                    >
                      <X className="w-4 h-4" />
                      <span>{t("settings.file.summary.cancel")}</span>
                    </Button>
                  </div>
                  {summaryDownloading && summaryProgress && (
                    <ProgressBar
                      size="full"
                      progress={[
                        {
                          id: summaryProgress.phase,
                          percentage: summaryProgress.current,
                        },
                      ]}
                    />
                  )}
                </div>
              )}

              {summaryStatus === "error" && summaryError && (
                <Alert variant="error">
                  {describeSummaryError(summaryError)}
                </Alert>
              )}

              {summaryMarkdown === null ? (
                <div className="border border-mid-gray/20 rounded-lg p-4 max-h-[60vh] overflow-y-auto">
                  <MarkdownContent markdown={markdown} />
                </div>
              ) : (
                <div className="space-y-4">
                  <div className="space-y-2">
                    <h3 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
                      {t("settings.file.summary.transcriptionTitle")}
                    </h3>
                    <div className="border border-mid-gray/20 rounded-lg p-4 max-h-[60vh] overflow-y-auto">
                      <MarkdownContent markdown={markdown} />
                    </div>
                  </div>
                  <div className="space-y-2">
                    <h3 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
                      {t("settings.file.summary.title")}
                    </h3>
                    <div className="border border-logo-primary/30 rounded-lg p-4 max-h-[60vh] overflow-y-auto">
                      <MarkdownContent markdown={summaryMarkdown} />
                    </div>
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      </div>

      {/* Historique persistant des transcriptions de fichiers et d'URL. */}
      {history.length > 0 && (
        <div className="space-y-2">
          <div className="px-4">
            <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
              {t("settings.file.history.title")}
            </h2>
          </div>
          <div className="bg-background border border-mid-gray/20 rounded-lg divide-y divide-mid-gray/10">
            {history.map((item) => (
              <div
                key={item.id}
                className={`flex items-center gap-2 px-4 py-2.5 transition-colors hover:bg-mid-gray/5 ${
                  item.id === historyId ? "bg-logo-primary/5" : ""
                }`}
              >
                <button
                  type="button"
                  onClick={() => void handleOpenHistoryEntry(item.id)}
                  disabled={busy}
                  className="flex items-center gap-3 flex-1 min-w-0 text-left disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {item.source_kind === "url" ? (
                    <Link2 className="w-4 h-4 shrink-0 text-text/40" />
                  ) : (
                    <FileAudio className="w-4 h-4 shrink-0 text-text/40" />
                  )}
                  <div className="flex-1 min-w-0">
                    <p className="text-sm font-medium text-text truncate">
                      {item.source_name}
                    </p>
                    <p className="text-xs text-text/50 truncate">
                      {item.snippet}
                    </p>
                  </div>
                  <span className="text-xs text-text/40 whitespace-nowrap">
                    {formatDate(item.created_at)}
                  </span>
                </button>
                {item.source_kind === "url" &&
                  (videoDownloads[item.id] !== undefined ? (
                    <span className="flex items-center gap-1 text-xs text-text/50 whitespace-nowrap shrink-0">
                      <Loader2 className="w-3.5 h-3.5 animate-spin" />
                      {videoDownloads[item.id]}%
                    </span>
                  ) : (
                    <button
                      type="button"
                      onClick={() =>
                        void handleSaveVideo(item.id, item.source_name)
                      }
                      title={t("settings.file.video.save")}
                      className={`p-1.5 rounded shrink-0 transition-colors ${
                        item.has_video
                          ? "text-logo-primary hover:bg-logo-primary/10"
                          : "text-text/40 hover:text-logo-primary hover:bg-logo-primary/10"
                      }`}
                    >
                      <Film className="w-4 h-4" />
                    </button>
                  ))}
                {/* Supprimer pendant une transcription ou un résumé
                    retirerait l'entrée que le calcul en cours alimente. */}
                <button
                  type="button"
                  onClick={() =>
                    void handleDeleteHistoryEntry(item.id, item.source_name)
                  }
                  disabled={busy}
                  title={t("settings.file.history.delete")}
                  className="p-1.5 rounded shrink-0 text-text/40 hover:text-red-500 hover:bg-red-500/10 transition-colors disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:text-text/40 disabled:hover:bg-transparent"
                >
                  <Trash2 className="w-4 h-4" />
                </button>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
};
