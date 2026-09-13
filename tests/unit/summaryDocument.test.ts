import { describe, expect, test } from "bun:test";
import {
  buildExportMarkdown,
  formatGigabytes,
  markdownToHtml,
} from "../../src/lib/utils/summaryDocument";

describe("buildExportMarkdown", () => {
  test("transcription seule", () => {
    expect(buildExportMarkdown("Bonjour.  ", null)).toBe("Bonjour.\n");
  });
  test("transcription puis --- puis compte-rendu", () => {
    expect(buildExportMarkdown("Bonjour.", "# Titre\n\n## Résumé\nx\n")).toBe(
      "Bonjour.\n\n---\n\n# Titre\n\n## Résumé\nx\n",
    );
  });
  test("compte-rendu vide ignoré", () => {
    expect(buildExportMarkdown("Bonjour.", "   ")).toBe("Bonjour.\n");
  });
});

describe("markdownToHtml", () => {
  test("titres, listes et gras", () => {
    const html = markdownToHtml(
      "# Titre\n\n## Résumé\nUn **mot**.\n\n## Points clés\n- a\n- b\n",
    );
    expect(html).toContain("<h1>Titre</h1>");
    expect(html).toContain("<h2>Résumé</h2>");
    expect(html).toContain("<strong>mot</strong>");
    expect(html).toContain("<ul>");
    expect(html).toContain("<li>a</li>");
  });
  // Même configuration que `MarkdownContent` (`skipHtml`) : le HTML brut écrit
  // dans le Markdown est retiré, et non recopié sous forme échappée — ce qui
  // est collé dans un traitement de texte est ce qui est affiché à l'écran.
  test("le HTML brut est retiré, jamais interprété", () => {
    expect(markdownToHtml("<script>x</script>")).toBe("");
    expect(
      markdownToHtml("Texte <b>gras</b> et <script>alert(1)</script> mêlés."),
    ).toBe("<p>Texte gras et alert(1) mêlés.</p>");
  });
  // Les éléments du gabarit du compte-rendu font tous partie de la liste
  // autorisée : rien n'est perdu à la copie.
  test("tableaux GFM et citations conservés", () => {
    const html = markdownToHtml(
      "> Citation\n\n| a | b |\n| - | - |\n| 1 | 2 |\n",
    );
    expect(html).toContain("<blockquote>");
    expect(html).toContain("<table>");
    expect(html).toContain("<td>1</td>");
  });
});

describe("formatGigabytes", () => {
  test("format français à une décimale", () => {
    expect(formatGigabytes(1_200_000_000)).toBe("1,2 Go");
    expect(formatGigabytes(0)).toBe("0 Go");
  });
});
