import { describe, expect, test } from "bun:test";
import {
  exportFileName,
  formatClock,
  formatDuration,
  formatSize,
  meetingErrorMessage,
  sortRecordings,
} from "../../src/components/meeting/format";
import type { MeetingRecording } from "../../src/bindings";

// `t` factice : renvoie la clé suivie de ses paramètres, pour vérifier le
// choix du texte.
const t = ((key: string, options?: Record<string, unknown>) =>
  options ? `${key} ${JSON.stringify(options)}` : key) as never;

const recording = (
  id: number,
  name: string,
  createdAt: number,
  durationMs = 0,
): MeetingRecording => ({
  id,
  name,
  createdAt,
  durationMs,
  sizeBytes: 0,
  state: "done",
  parts: 1,
});

describe("formatClock", () => {
  test("affiche heures, minutes et secondes sur deux chiffres", () => {
    expect(formatClock(0)).toBe("00:00:00");
    expect(formatClock(247_900)).toBe("00:04:07");
    expect(formatClock(12.5 * 3_600_000)).toBe("12:30:00");
  });

  test("dépasse 24 h sans revenir à zéro", () => {
    expect(formatClock(25 * 3_600_000)).toBe("25:00:00");
  });
});

describe("formatDuration", () => {
  test("choisit l'unité lisible", () => {
    expect(formatDuration(45_000)).toBe("45 s");
    expect(formatDuration(12 * 60_000 + 20_000)).toBe("12 min");
    expect(formatDuration(65 * 60_000)).toBe("1 h 05 min");
    expect(formatDuration(24 * 3_600_000)).toBe("24 h");
  });
});

describe("formatSize", () => {
  test("utilise les unités décimales françaises", () => {
    expect(formatSize(820_000)).toBe("820 Ko");
    expect(formatSize(115_200_000)).toBe("115 Mo");
    expect(formatSize(2_764_800_000)).toBe("2,8 Go");
  });
});

describe("sortRecordings", () => {
  const list = [
    recording(1, "Comité de pilotage", 100, 3_600_000),
    recording(2, "atelier client", 300, 600_000),
    recording(3, "Équipe produit", 200, 7_200_000),
  ];

  test("trie par date, nom ou durée", () => {
    const ids = (sorted: MeetingRecording[]) => sorted.map((r) => r.id);
    expect(ids(sortRecordings(list, "newest", ""))).toEqual([2, 3, 1]);
    expect(ids(sortRecordings(list, "oldest", ""))).toEqual([1, 3, 2]);
    expect(ids(sortRecordings(list, "name", ""))).toEqual([2, 1, 3]);
    expect(ids(sortRecordings(list, "longest", ""))).toEqual([3, 1, 2]);
  });

  test("filtre sans tenir compte des accents ni de la casse", () => {
    expect(sortRecordings(list, "newest", "EQUIPE").map((r) => r.id)).toEqual([
      3,
    ]);
    expect(sortRecordings(list, "newest", "  ")).toHaveLength(3);
  });

  test("ne modifie pas la liste reçue", () => {
    sortRecordings(list, "name", "");
    expect(list.map((r) => r.id)).toEqual([1, 2, 3]);
  });
});

describe("exportFileName", () => {
  test("retire les caractères interdits par Windows", () => {
    expect(exportFileName("Réunion du 24/10/2026 à 14h05")).toBe(
      "Réunion du 24-10-2026 à 14h05.wav",
    );
    expect(exportFileName('Point "budget" : Q4?')).toBe(
      "Point -budget- - Q4-.wav",
    );
    expect(exportFileName("   ")).toBe("reunion.wav");
    expect(exportFileName("Fin.")).toBe("Fin.wav");
  });
});

describe("meetingErrorMessage", () => {
  test("formate les tailles et choisit la clé de l'erreur", () => {
    expect(
      meetingErrorMessage(t, { kind: "diskSpace", freeBytes: 300_000_000 }),
    ).toBe('settings.meeting.errors.diskSpace {"free":"300 Mo"}');
    expect(meetingErrorMessage(t, { kind: "microphonePermission" })).toBe(
      "settings.meeting.errors.microphonePermission",
    );
    expect(
      meetingErrorMessage(t, { kind: "microphoneFailed", detail: "WASAPI" }),
    ).toBe("settings.meeting.errors.microphoneFailed");
  });
});
