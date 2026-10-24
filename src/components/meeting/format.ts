import type { TFunction } from "i18next";
import type { MeetingError, MeetingRecording, TextAssetKind } from "@/bindings";

// Fonctions pures de l'onglet Réunion (testées dans tests/unit) : affichage
// des durées et tailles en typographie française, tri de la liste, messages
// d'erreur.

const pad = (value: number): string => value.toString().padStart(2, "0");

/** Chronomètre : « 00:04:07 », « 12:30:00 », « 25:00:00 ». */
export const formatClock = (ms: number): string => {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  return `${pad(hours)}:${pad(minutes)}:${pad(seconds)}`;
};

/** Durée lisible : « 45 s », « 12 min », « 1 h 05 min », « 24 h ». */
export const formatDuration = (ms: number): string => {
  const totalSeconds = Math.max(0, Math.round(ms / 1000));
  if (totalSeconds < 60) return `${totalSeconds} s`;
  const totalMinutes = Math.floor(totalSeconds / 60);
  if (totalMinutes < 60) return `${totalMinutes} min`;
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  return minutes === 0 ? `${hours} h` : `${hours} h ${pad(minutes)} min`;
};

const sizeFormatter = (digits: number) =>
  new Intl.NumberFormat("fr-FR", { maximumFractionDigits: digits });

/** Taille en unités décimales : « 820 Ko », « 115 Mo », « 2,8 Go ». */
export const formatSize = (bytes: number): string => {
  if (bytes >= 1_000_000_000) {
    return `${sizeFormatter(1).format(bytes / 1_000_000_000)} Go`;
  }
  if (bytes >= 1_000_000) {
    return `${sizeFormatter(0).format(bytes / 1_000_000)} Mo`;
  }
  return `${sizeFormatter(0).format(Math.max(0, bytes) / 1_000)} Ko`;
};

export type RecordingSort = "newest" | "oldest" | "name" | "longest";

const normalize = (text: string): string =>
  text.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();

/** Liste filtrée (nom, sans tenir compte des accents ni de la casse) et triée. */
export const sortRecordings = (
  recordings: MeetingRecording[],
  sort: RecordingSort,
  query: string,
): MeetingRecording[] => {
  const needle = normalize(query.trim());
  const filtered = needle
    ? recordings.filter((r) => normalize(r.name).includes(needle))
    : [...recordings];
  const byDate = (a: MeetingRecording, b: MeetingRecording) =>
    a.createdAt - b.createdAt || a.id - b.id;
  switch (sort) {
    case "newest":
      return filtered.sort((a, b) => byDate(b, a));
    case "oldest":
      return filtered.sort(byDate);
    case "name":
      return filtered.sort(
        (a, b) =>
          a.name.localeCompare(b.name, "fr", { sensitivity: "base" }) ||
          byDate(b, a),
      );
    case "longest":
      return filtered.sort(
        (a, b) => b.durationMs - a.durationMs || byDate(b, a),
      );
  }
};

/** Nom de fichier proposé à l'export : le nom de l'enregistrement, sans les
 *  caractères interdits par Windows. */
export const exportFileName = (name: string): string => {
  const safe = name
    .replace(/[<>:"/\\|?*]/g, "-")
    .split("")
    .filter((c) => c.charCodeAt(0) >= 32)
    .join("")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/[. ]+$/, "");
  return `${safe || "reunion"}.wav`;
};

export const meetingErrorMessage = (
  t: TFunction,
  error: MeetingError,
): string => {
  switch (error.kind) {
    case "diskSpace":
      return t("settings.meeting.errors.diskSpace", {
        free: formatSize(error.freeBytes),
      });
    case "fileTooLarge":
      return t("settings.meeting.errors.fileTooLarge", {
        max: formatSize(error.maxBytes),
      });
    case "microphoneFailed":
      return t("settings.meeting.errors.microphoneFailed");
    case "invalidFile":
      return t("settings.meeting.errors.invalidFile", {
        detail: error.detail,
      });
    case "storage":
      return t("settings.meeting.errors.storage", { detail: error.detail });
    default:
      return t(`settings.meeting.errors.${error.kind}`);
  }
};

/** Longueur au-delà de laquelle le modèle local du compte rendu en néglige
 *  une partie et ralentit (banc d'essai, docs/compte-rendu-reunion-faisabilite.md) :
 *  ~1 000 tokens pour la charte, ~800 pour le skill. */
export const TEXT_ASSET_WORD_LIMITS: Record<TextAssetKind, number> = {
  editorial: 650,
  skill: 500,
};
