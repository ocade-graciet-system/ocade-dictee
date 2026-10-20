import { describe, expect, test } from "bun:test";
import {
  bannerFor,
  checkChar,
  contactMailto,
  daysLabel,
  formatKeyInput,
  isCompleteKey,
  licenceErrorMessage,
  normalizeKey,
  stateTone,
} from "../../src/components/licence/format";
import type { LicenceView } from "../../src/bindings";

// `t` factice : renvoie la clé et ses paramètres, pour vérifier le choix du
// texte. Seule la clé `licence.errors.unknown_code` est « absente » et renvoie
// donc la valeur par défaut, comme le ferait i18next.
const t = ((key: string, options?: Record<string, unknown>) => {
  if (options && "defaultValue" in options) {
    return key === "licence.errors.unknown_code" ? options.defaultValue : key;
  }
  return options ? `${key} ${JSON.stringify(options)}` : key;
}) as never;

const baseView: LicenceView = {
  state: "active",
  mode: "soft",
  blocking: false,
  enforced: false,
  key_suffix: "3RCP",
  expires_at: 1_800_000_000,
  days_left: 200,
  token_expires_at: 1_790_000_000,
  seats_max: 1,
  seats_used: 1,
  hostname: "PC-TEST",
  last_check_at: 1_780_000_000,
  refusal: null,
};

// Vecteurs vérifiés côté serveur (ocade-dictee-licences) et côté Rust.
const VECTORS: [string, string, string][] = [
  ["ABCDEFGHJKLMNPQ", "9", "OCAD-ABCD-EFGH-JKLM-NPQ9"],
  ["222222222222222", "S", "OCAD-2222-2222-2222-222S"],
  ["ZZZZZZZZZZZZZZZ", "H", "OCAD-ZZZZ-ZZZZ-ZZZZ-ZZZH"],
  ["7XQ2M9KDT4HW3RC", "P", "OCAD-7XQ2-M9KD-T4HW-3RCP"],
  ["AAAAAAAAAAAAAAA", "A", "OCAD-AAAA-AAAA-AAAA-AAAA"],
  ["23456789ABCDEFG", "V", "OCAD-2345-6789-ABCD-EFGV"],
];

describe("normalizeKey — même règle que le serveur", () => {
  test("caractère de contrôle Luhn mod 32 des vecteurs partagés", () => {
    for (const [body, check] of VECTORS) expect(checkChar(body)).toBe(check);
  });
  test("chaque vecteur est accepté, quelle que soit la saisie", () => {
    for (const [, , key] of VECTORS) {
      expect(normalizeKey(key)).toBe(key);
      expect(normalizeKey(key.toLowerCase().replace(/-/g, " "))).toBe(key);
      expect(normalizeKey(key.slice(5))).toBe(key);
    }
  });
  test("deux symboles intervertis sont refusés", () => {
    expect(normalizeKey("OCAD-7XQ2-M9KD-T4HW-3CRP")).toBeNull();
  });
  test("symbole hors alphabet, longueur fausse, préfixe seul", () => {
    expect(normalizeKey("OCAD-7XQ2-M9KD-T4HW-3RC0")).toBeNull();
    expect(normalizeKey("OCAD-7XQ2-M9KD")).toBeNull();
    expect(normalizeKey("OCAD-7XQ2-M9KD-T4HW")).toBeNull();
  });
});

describe("formatKeyInput — saisie de la clé", () => {
  test("met en majuscules, ajoute le préfixe et les tirets", () => {
    expect(formatKeyInput("7xq2m9")).toBe("OCAD-7XQ2-M9");
    expect(formatKeyInput("ocad 7xq2 m9kd t4hw 3rcp")).toBe(
      "OCAD-7XQ2-M9KD-T4HW-3RCP",
    );
  });
  test("accepte une clé collée telle quelle et coupe l'excédent", () => {
    expect(formatKeyInput("OCAD-7XQ2-M9KD-T4HW-3RCP")).toBe(
      "OCAD-7XQ2-M9KD-T4HW-3RCP",
    );
    expect(formatKeyInput("OCAD-7XQ2-M9KD-T4HW-3RCPZZZ")).toBe(
      "OCAD-7XQ2-M9KD-T4HW-3RCP",
    );
    expect(formatKeyInput("")).toBe("");
  });
  test("une clé est complète à 16 symboles", () => {
    expect(isCompleteKey("OCAD-7XQ2-M9KD-T4HW-3RCP")).toBe(true);
    expect(isCompleteKey("OCAD-7XQ2-M9KD-T4HW-3RC")).toBe(false);
  });
});

describe("textes de la licence", () => {
  test("jours restants", () => {
    expect(daysLabel(t, 0)).toBe("licence.days.none");
    expect(daysLabel(t, 1)).toBe("licence.days.one");
    expect(daysLabel(t, 12)).toContain("licence.days.many");
  });
  test("une erreur inconnue retombe sur le message générique", () => {
    const err = {
      code: "unknown_code",
      seats_used: null,
      seats_max: null,
      expired_at: null,
      detail: null,
    };
    expect(licenceErrorMessage(t, err)).toBe("licence.errors.generic");
  });
  test("postes complets : le message cite les compteurs", () => {
    const err = {
      code: "seats_full",
      seats_used: 2,
      seats_max: 2,
      expired_at: null,
      detail: null,
    };
    expect(licenceErrorMessage(t, err)).toContain("seatsFullCount");
  });
});

describe("bannerFor — rappels dans l'application", () => {
  test("rien quand tout va bien ou quand l'écran de blocage est affiché", () => {
    expect(bannerFor(t, baseView)).toBeNull();
    expect(
      bannerFor(t, { ...baseView, state: "revoked", enforced: true }),
    ).toBeNull();
  });
  test("information à 30 jours, alerte à 7 jours, alerte hors ligne", () => {
    expect(bannerFor(t, { ...baseView, state: "expiring_soon" })?.tone).toBe(
      "info",
    );
    expect(
      bannerFor(t, { ...baseView, state: "expiring_critical", days_left: 5 })
        ?.tone,
    ).toBe("warning");
    expect(bannerFor(t, { ...baseView, state: "offline_grace" })?.tone).toBe(
      "warning",
    );
  });
  test("refus en mode souple : bandeau d'erreur, l'application reste utilisable", () => {
    const banner = bannerFor(t, { ...baseView, state: "expired" });
    expect(banner?.tone).toBe("error");
  });
  test("couleur de la pastille", () => {
    expect(stateTone("active")).toBe("ok");
    expect(stateTone("offline_grace")).toBe("warning");
    expect(stateTone("not_activated")).toBe("neutral");
    expect(stateTone("unknown_key")).toBe("error");
  });
});

describe("contactMailto — e-mail prérempli vers OCADE", () => {
  test("destinataire, objet et informations du poste", () => {
    const url = contactMailto(t, baseView, "1.8.0");
    expect(url.startsWith("mailto:contact@ocadefusion.fr?subject=")).toBe(true);
    const body = decodeURIComponent(url.split("&body=")[1] ?? "");
    expect(body).toContain("3RCP");
    expect(body).toContain("PC-TEST");
    expect(body).toContain("1.8.0");
  });
});
