/**
 * Mise en forme d'une combinaison de touches pour l'affichage, et
 * normalisation d'une combinaison capturée par l'enregistreur handy-keys.
 *
 * Les chaînes manipulées sont celles du backend : jetons en minuscules séparés
 * par « + », graphie handy-keys (`option`/`alt`, `command`/`super`, `left`,
 * `return`, `escape`, `keypad1`…), éventuellement latéralisées (`ctrl_left`).
 */

import type { OSType } from "./keyboard";

const MODIFIER_LABELS: Record<string, { mac: string; other: string }> = {
  ctrl: { mac: "⌃", other: "Ctrl" },
  control: { mac: "⌃", other: "Ctrl" },
  option: { mac: "⌥", other: "Alt" },
  opt: { mac: "⌥", other: "Alt" },
  alt: { mac: "⌥", other: "Alt" },
  shift: { mac: "⇧", other: "Maj" },
  command: { mac: "⌘", other: "Win" },
  cmd: { mac: "⌘", other: "Win" },
  meta: { mac: "⌘", other: "Win" },
  super: { mac: "⌘", other: "Win" },
  win: { mac: "⌘", other: "Win" },
  windows: { mac: "⌘", other: "Win" },
  fn: { mac: "fn", other: "Fn" },
};

const KEY_LABELS: Record<string, string> = {
  space: "Espace",
  return: "Entrée",
  enter: "Entrée",
  tab: "Tab",
  escape: "Échap",
  esc: "Échap",
  delete: "Retour arrière",
  backspace: "Retour arrière",
  forwarddelete: "Suppr",
  del: "Suppr",
  insert: "Inser",
  home: "Début",
  end: "Fin",
  pageup: "Page préc.",
  pagedown: "Page suiv.",
  left: "←",
  leftarrow: "←",
  right: "→",
  rightarrow: "→",
  up: "↑",
  uparrow: "↑",
  down: "↓",
  downarrow: "↓",
  capslock: "Verr. maj.",
  numlock: "Verr. num.",
  scrolllock: "Arrêt défil.",
  dictation: "Dictée",
  // Noms longs des touches de ponctuation : handy-keys accepte « comma »
  // aussi bien que « , », et une valeur héritée peut être écrite de l'une
  // ou l'autre façon.
  comma: ",",
  period: ".",
  semicolon: ";",
  quote: "'",
  slash: "/",
  backslash: "\\",
  minus: "-",
  equal: "=",
  equals: "=",
  leftbracket: "[",
  rightbracket: "]",
  grave: "`",
  backtick: "`",
  section: "§",
};

const KEYPAD_LABELS: Record<string, string> = {
  plus: "+",
  minus: "−",
  multiply: "×",
  divide: "÷",
  decimal: ".",
  comma: ",",
  equals: "=",
  enter: "Entrée",
  clear: "Effacer",
};

/** Retire le suffixe de latéralité d'un jeton (`option_left` → `option`). */
const delateralize = (token: string): string =>
  token.replace(/_(left|right)$/, "");

const labelForToken = (token: string, isMac: boolean): string => {
  const modifier = MODIFIER_LABELS[token];
  if (modifier) return isMac ? modifier.mac : modifier.other;

  const key = KEY_LABELS[token];
  if (key) return key;

  // f1 → F1
  if (/^f\d{1,2}$/.test(token)) return token.toUpperCase();

  // keypad1 → « Pavé 1 », keypadplus → « Pavé + »
  if (token.startsWith("keypad")) {
    const rest = token.slice("keypad".length);
    return `Pavé ${KEYPAD_LABELS[rest] ?? rest}`.trim();
  }

  // a → A, « , » → « , »
  if (token.length === 1) return token.toUpperCase();

  return token.charAt(0).toUpperCase() + token.slice(1);
};

/**
 * « ctrl+option+space » → « ⌃ ⌥ Espace » (macOS) ou « Ctrl + Alt + Espace ».
 * Sert à la fois aux préréglages et au raccourci personnalisé.
 */
export const formatShortcut = (binding: string, os: OSType): string => {
  if (!binding) return "";
  const isMac = os === "macos";
  const parts = binding
    .split("+")
    .map((token) => token.trim().toLowerCase())
    .filter((token) => token.length > 0)
    .map((token) => labelForToken(delateralize(token), isMac));
  return isMac ? parts.join(" ") : parts.join(" + ");
};

/**
 * Combinaison capturée par l'enregistreur handy-keys → forme à enregistrer.
 * La capture latéralise les modificateurs d'après les drapeaux matériels
 * (« ctrl_left+option_left+d ») ; un raccourci latéralisé ne se déclenche
 * qu'avec les touches de ce côté du clavier. On dé-latéralise donc, comme les
 * préréglages.
 */
export const normalizeCapturedShortcut = (hotkeyString: string): string =>
  hotkeyString
    .split("+")
    .map((token) => delateralize(token.trim().toLowerCase()))
    .filter((token) => token.length > 0)
    .join("+");
