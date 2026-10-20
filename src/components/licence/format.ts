import type { TFunction } from "i18next";
import type { LicenceError, LicenceState, LicenceView } from "@/bindings";

const KEY_SYMBOLS = /[^A-Z0-9]/g;

/** Alphabet des clés : 32 symboles, sans I, O, 0 ni 1. */
export const KEY_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const PREFIX = "OCAD";

/**
 * Caractère de contrôle Luhn mod 32 de 15 symboles. Même algorithme que le
 * serveur (src/domain/licenseKey.ts) et que le module Rust (licence/key.rs).
 * `null` si un symbole n'appartient pas à l'alphabet.
 */
export function checkChar(body: string): string | null {
  const n = KEY_ALPHABET.length;
  let factor = 2;
  let sum = 0;
  for (let i = body.length - 1; i >= 0; i--) {
    const cp = KEY_ALPHABET.indexOf(body[i] ?? "");
    if (cp < 0) return null;
    let addend = factor * cp;
    factor = factor === 2 ? 1 : 2;
    addend = Math.floor(addend / n) + (addend % n);
    sum += addend;
  }
  return KEY_ALPHABET[(n - (sum % n)) % n] ?? null;
}

/**
 * Normalise une saisie comme le serveur : majuscules, sans espaces ni tirets,
 * préfixe retiré seulement s'il reste 16 symboles, alphabet et contrôle
 * vérifiés. Renvoie `OCAD-XXXX-XXXX-XXXX-XXXX`, ou `null` si la clé est fausse.
 */
export function normalizeKey(input: string): string | null {
  let s = input.toUpperCase().replace(/[\s-]/g, "");
  if (s.startsWith(PREFIX) && s.length === PREFIX.length + 16) {
    s = s.slice(PREFIX.length);
  }
  if (s.length !== 16) return null;
  if ([...s].some((ch) => !KEY_ALPHABET.includes(ch))) return null;
  if (checkChar(s.slice(0, 15)) !== s[15]) return null;
  return [PREFIX, ...(s.match(/.{4}/g) ?? [])].join("-");
}

/**
 * Met en forme la saisie d'une clé au fil de la frappe : majuscules, préfixe
 * `OCAD-` et tirets tous les 4 symboles. La validité est vérifiée à part
 * ([`normalizeKey`]), avant tout appel réseau.
 */
export function formatKeyInput(raw: string): string {
  let compact = raw.toUpperCase().replace(KEY_SYMBOLS, "");
  if (compact.startsWith(PREFIX)) compact = compact.slice(PREFIX.length);
  compact = compact.slice(0, 16);
  if (!compact) return "";
  const groups = compact.match(/.{1,4}/g) ?? [];
  return `${PREFIX}-${groups.join("-")}`;
}

/** Une clé complète a 16 symboles après le préfixe. */
export function isCompleteKey(formatted: string): boolean {
  return formatted.replace(/^OCAD-/, "").replace(/-/g, "").length === 16;
}

/** Date en français, à partir de secondes Unix. */
export function formatDate(seconds: number): string {
  return new Date(seconds * 1000).toLocaleDateString("fr-FR", {
    day: "numeric",
    month: "long",
    year: "numeric",
  });
}

export function formatDateTime(seconds: number): string {
  return new Date(seconds * 1000).toLocaleString("fr-FR", {
    day: "numeric",
    month: "long",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Adresse de contact affichée aux utilisateurs pour toute question de licence. */
export const CONTACT_EMAIL = "contact@ocadefusion.fr";

/**
 * Lien `mailto:` vers OCADE, avec un e-mail prérempli : ordinateur, suffixe de
 * la clé, état et version, pour identifier le poste sans aller-retour.
 */
export function contactMailto(
  t: TFunction,
  view: LicenceView | null,
  version: string,
): string {
  const body = t("licence.contact.body", {
    computer: view?.hostname ?? "",
    key: view?.key_suffix
      ? `OCAD-…-${view.key_suffix}`
      : t("licence.contact.noKey"),
    state: view ? t(`licence.states.${view.state}`) : "",
    version,
    interpolation: { escapeValue: false },
  });
  const subject = t("licence.contact.subject");
  return `mailto:${CONTACT_EMAIL}?subject=${encodeURIComponent(subject)}&body=${encodeURIComponent(body)}`;
}

export function daysLabel(t: TFunction, days: number): string {
  if (days <= 0) return t("licence.days.none");
  if (days === 1) return t("licence.days.one");
  return t("licence.days.many", { count: days });
}

/** Message français pour une erreur renvoyée par une commande de licence. */
export function licenceErrorMessage(t: TFunction, err: LicenceError): string {
  if (err.code === "expired" && err.expired_at !== null) {
    return t("licence.errors.expiredOn", { date: formatDate(err.expired_at) });
  }
  if (
    err.code === "seats_full" &&
    err.seats_used !== null &&
    err.seats_max !== null
  ) {
    return t("licence.errors.seatsFullCount", {
      used: err.seats_used,
      max: err.seats_max,
    });
  }
  const message = t(`licence.errors.${err.code}`, { defaultValue: "" });
  return message !== "" ? message : t("licence.errors.generic");
}

export type Tone = "ok" | "warning" | "error" | "neutral";

/** Couleur de la pastille d'état. */
export function stateTone(state: LicenceState): Tone {
  switch (state) {
    case "active":
    case "development":
      return "ok";
    case "expiring_soon":
    case "expiring_critical":
    case "offline_grace":
      return "warning";
    case "not_activated":
      return "neutral";
    default:
      return "error";
  }
}

export interface BannerContent {
  tone: "info" | "warning" | "error";
  text: string;
}

/**
 * Bandeau à afficher en haut de l'application, ou `null`. Quand l'état est
 * bloquant en mode strict, c'est l'écran de blocage qui s'affiche à la place.
 */
export function bannerFor(
  t: TFunction,
  view: LicenceView,
): BannerContent | null {
  if (view.enforced) return null;
  switch (view.state) {
    case "development":
    case "active":
      return null;
    case "not_activated":
      return { tone: "info", text: t("licence.banner.notActivated") };
    case "expiring_soon":
      return view.expires_at === null
        ? null
        : {
            tone: "info",
            text: t("licence.banner.soon", {
              date: formatDate(view.expires_at),
            }),
          };
    case "expiring_critical":
      return view.expires_at === null
        ? null
        : {
            tone: "warning",
            text: t("licence.banner.critical", {
              date: formatDate(view.expires_at),
              days: daysLabel(t, view.days_left ?? 0).toLowerCase(),
            }),
          };
    case "offline_grace":
      return view.token_expires_at === null
        ? null
        : {
            tone: "warning",
            text: t("licence.banner.offline", {
              date: formatDate(view.token_expires_at),
            }),
          };
    default:
      return {
        tone: "error",
        text: t("licence.banner.invalid", {
          state: t(`licence.states.${view.state}`).toLowerCase(),
        }),
      };
  }
}
