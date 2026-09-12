import { test, expect } from "@playwright/test";
import {
  formatShortcut,
  normalizeCapturedShortcut,
} from "../src/lib/utils/shortcutFormat";

test.describe("formatShortcut", () => {
  test("garde la mise en forme des préréglages macOS", () => {
    expect(formatShortcut("ctrl+option+space", "macos")).toBe("⌃ ⌥ Espace");
    expect(formatShortcut("ctrl+shift+d", "macos")).toBe("⌃ ⇧ D");
  });

  test("utilise des libellés sur Windows et Linux", () => {
    expect(formatShortcut("ctrl+alt+space", "windows")).toBe(
      "Ctrl + Alt + Espace",
    );
    expect(formatShortcut("ctrl+shift+d", "linux")).toBe("Ctrl + Maj + D");
  });

  test("met en forme touches de fonction, flèches et ponctuation", () => {
    expect(formatShortcut("ctrl+f8", "macos")).toBe("⌃ F8");
    expect(formatShortcut("ctrl+left", "macos")).toBe("⌃ ←");
    expect(formatShortcut("ctrl+alt+enter", "windows")).toBe(
      "Ctrl + Alt + Entrée",
    );
    expect(formatShortcut("cmd+comma", "macos")).toBe("⌘ ,");
    expect(formatShortcut("ctrl+alt+keypad1", "linux")).toBe(
      "Ctrl + Alt + Pavé 1",
    );
  });

  test("ignore les suffixes de latéralité à l'affichage", () => {
    expect(formatShortcut("ctrl_left+option_right+d", "macos")).toBe("⌃ ⌥ D");
  });

  test("renvoie une chaîne vide pour une combinaison vide", () => {
    expect(formatShortcut("", "macos")).toBe("");
  });
});

test.describe("normalizeCapturedShortcut", () => {
  test("dé-latéralise les modificateurs capturés", () => {
    expect(normalizeCapturedShortcut("ctrl_left+option_left+d")).toBe(
      "ctrl+option+d",
    );
    expect(normalizeCapturedShortcut("shift_right+cmd_left+space")).toBe(
      "shift+cmd+space",
    );
  });

  test("laisse intacte une combinaison déjà normalisée", () => {
    expect(normalizeCapturedShortcut("ctrl+shift+space")).toBe(
      "ctrl+shift+space",
    );
  });

  test("supprime les espaces et les jetons vides", () => {
    expect(normalizeCapturedShortcut(" ctrl + option + d ")).toBe(
      "ctrl+option+d",
    );
  });
});
