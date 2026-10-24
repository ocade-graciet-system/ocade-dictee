import { readFile } from "@tauri-apps/plugin-fs";

const MIME: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  webp: "image/webp",
  gif: "image/gif",
  svg: "image/svg+xml",
};

/** Largeur maximale du PNG envoyé au document Word (le logo y fait 6 cm au
 *  plus : 800 px suffisent largement). */
const MAX_WIDTH = 800;

export interface LogoPng {
  pngBase64: string;
  width: number;
  height: number;
}

/**
 * Convertit le logo de la charte (PNG, JPG, WebP, GIF ou SVG) en PNG pour le
 * document Word, qui ne lit ni le WebP ni le SVG seul. Le fichier est lu
 * octet par octet puis dessiné depuis une URL `blob:` : une image servie par
 * le protocole `asset:` rendrait le canevas illisible (origine différente).
 */
export async function logoToPng(path: string): Promise<LogoPng | null> {
  const extension = path.split(".").pop()?.toLowerCase() ?? "";
  const bytes = await readFile(path);
  const url = URL.createObjectURL(
    new Blob([bytes], { type: MIME[extension] ?? "image/png" }),
  );
  try {
    const image = new Image();
    image.src = url;
    await image.decode();
    // Un SVG sans dimensions intrinsèques : format bandeau par défaut.
    const naturalWidth = image.naturalWidth || 600;
    const naturalHeight = image.naturalHeight || 200;
    const scale = Math.min(1, MAX_WIDTH / naturalWidth);
    const width = Math.max(1, Math.round(naturalWidth * scale));
    const height = Math.max(1, Math.round(naturalHeight * scale));
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d");
    if (!context) return null;
    context.drawImage(image, 0, 0, width, height);
    const dataUrl = canvas.toDataURL("image/png");
    return {
      pngBase64: dataUrl.slice(dataUrl.indexOf(",") + 1),
      width,
      height,
    };
  } finally {
    URL.revokeObjectURL(url);
  }
}
