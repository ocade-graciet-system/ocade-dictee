// Télécharge le binaire sidecar yt-dlp pour le target Tauri courant dans
// src-tauri/binaries/ sous le nom attendu par bundle.externalBin
// ("binaries/yt-dlp" -> yt-dlp-<triple>[.exe]). No-op si déjà présent.
//
// Appelé par beforeDevCommand / beforeBuildCommand (tauri.conf.json) : Tauri
// fournit TAURI_ENV_TARGET_TRIPLE (celui de --target, sinon l'hôte). Hors
// contexte Tauri, le triple hôte est déduit de process.platform/arch.
//
// yt-dlp est volontairement pris en "latest" : c'est un outil qui doit suivre
// les changements des sites vidéo ; figer une version le casserait à terme.

import { chmodSync, existsSync, mkdirSync, renameSync, statSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";

const BINARIES_DIR = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "src-tauri",
  "binaries",
);

// Assets officiels publiés par yt-dlp (binaires autonomes, sans Python).
// Pas de build Windows ARM natif : le x64 tourne via l'émulation Windows.
// yt-dlp_macos est un binaire universel (arm64 + x86_64).
const ASSETS: Record<string, string> = {
  "aarch64-apple-darwin": "yt-dlp_macos",
  "x86_64-apple-darwin": "yt-dlp_macos",
  "x86_64-pc-windows-msvc": "yt-dlp.exe",
  "aarch64-pc-windows-msvc": "yt-dlp.exe",
  "x86_64-unknown-linux-gnu": "yt-dlp_linux",
  "aarch64-unknown-linux-gnu": "yt-dlp_linux_aarch64",
};

function hostTriple(): string {
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  switch (process.platform) {
    case "darwin":
      return `${arch}-apple-darwin`;
    case "win32":
      return `${arch}-pc-windows-msvc`;
    default:
      return `${arch}-unknown-linux-gnu`;
  }
}

const triple = process.env.TAURI_ENV_TARGET_TRIPLE ?? hostTriple();

// macOS : pas de sidecar (externalBin est limité à Windows/Linux via les
// tauri.{windows,linux}.conf.json). La re-signature ad-hoc du bundle par Tauri
// casse yt-dlp_macos (PyInstaller) — « different Team IDs » au dlopen de sa
// bibliothèque Python. L'app télécharge le binaire au premier usage à la
// place (voir ensure_yt_dlp_macos dans file_transcription.rs).
if (triple.endsWith("apple-darwin")) {
  console.log(
    "fetch-yt-dlp: macOS — binaire géré par l'app au premier usage, rien à faire.",
  );
  process.exit(0);
}

const asset = ASSETS[triple];
if (!asset) {
  console.error(`fetch-yt-dlp: target triple non supporté: ${triple}`);
  process.exit(1);
}

const isWindows = triple.includes("windows");
const dest = path.join(
  BINARIES_DIR,
  `yt-dlp-${triple}${isWindows ? ".exe" : ""}`,
);

if (existsSync(dest) && statSync(dest).size > 0) {
  console.log(`fetch-yt-dlp: déjà présent (${path.basename(dest)})`);
  process.exit(0);
}

const url = `https://github.com/yt-dlp/yt-dlp/releases/latest/download/${asset}`;
console.log(`fetch-yt-dlp: téléchargement de ${url}`);

mkdirSync(BINARIES_DIR, { recursive: true });
const staging = `${dest}.download`;

// curl plutôt que fetch() : suit les redirections GitHub -> CDN de façon
// fiable, réessaie, et est présent sur toutes les plateformes visées (macOS,
// Windows 10+, Linux et les runners GitHub Actions).
const curl = Bun.spawnSync(
  [
    "curl",
    "-fL",
    "--retry",
    "3",
    "--connect-timeout",
    "30",
    "-o",
    staging,
    url,
  ],
  { stdout: "inherit", stderr: "inherit" },
);
if (curl.exitCode !== 0) {
  console.error(
    `fetch-yt-dlp: échec du téléchargement (curl code ${curl.exitCode})`,
  );
  process.exit(1);
}
renameSync(staging, dest);
if (!isWindows) {
  chmodSync(dest, 0o755);
}
console.log(
  `fetch-yt-dlp: OK -> ${path.basename(dest)} (${(statSync(dest).size / 1e6).toFixed(1)} Mo)`,
);
