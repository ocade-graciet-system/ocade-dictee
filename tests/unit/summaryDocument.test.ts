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
  test("le HTML brut n'est pas interprété", () => {
    expect(markdownToHtml("<script>x</script>")).not.toContain("<script>");
  });
});

describe("formatGigabytes", () => {
  test("format français à une décimale", () => {
    expect(formatGigabytes(1_200_000_000)).toBe("1,2 Go");
    expect(formatGigabytes(0)).toBe("0 Go");
  });
});
