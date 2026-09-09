import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { listen } from "@tauri-apps/api/event";
import { ProgressBar } from "../shared";
import { useModelStore } from "../../stores/modelStore";
import { useFileTranscriptionStore } from "../../stores/fileTranscriptionStore";
import { commands } from "@/bindings";

const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000; // toutes les 24 h (issue #6)
const RETRY_WHEN_BUSY_MS = 5 * 60 * 1000; // dictée ou téléchargement en cours → réessai dans 5 min
// Borne totale de la requête de téléchargement. Sans elle, un socket
// semi-ouvert ne rejette jamais et l'écran non fermable bloque l'application.
// Large à dessein : le bundle pèse quelques dizaines de Mo.
const DOWNLOAD_TIMEOUT_MS = 15 * 60 * 1000;

// Mise à jour forcée : aucune question, aucun report. Montée à la racine de
// l'app pour couvrir aussi les écrans de premier lancement.
export const ForcedUpdater: React.FC = () => {
  const { t } = useTranslation();
  const [update, setUpdate] = useState<Update | null>(null);
  const [progress, setProgress] = useState(0);
  // Le téléchargement et l'installation sont deux étapes distinctes, séparées
  // par une attente éventuelle : l'écran ne peut plus déduire l'étape du seul
  // pourcentage.
  const [phase, setPhase] = useState<"downloading" | "installing">(
    "downloading",
  );
  const busy = useRef(false);
  const retryTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const downloadedBytes = useRef(0);
  const totalBytes = useRef(0);
  // Paquet déjà téléchargé mais pas encore installé : gardé d'une tentative à
  // l'autre pour ne jamais retélécharger après un report.
  const downloaded = useRef<Update | null>(null);

  // Une dictée ne doit jamais être interrompue. Une préparation de modèle non
  // plus : elle reprendrait après le redémarrage, mais l'écran de préparation
  // du premier lancement disparaîtrait en plein transfert de 512 Mo. Une
  // transcription de fichier, elle, se compte en minutes et serait perdue.
  const isBusyElsewhere = async () => {
    const models = useModelStore.getState();
    return (
      (await commands.isRecording()) ||
      Object.keys(models.downloadingModels).length > 0 ||
      Object.keys(models.verifyingModels).length > 0 ||
      Object.keys(models.extractingModels).length > 0 ||
      useFileTranscriptionStore.getState().status === "processing"
    );
  };

  // Chaque `check()` réussi alloue une ressource côté Rust. Dès qu'on renonce à
  // l'installer, il faut la rendre : sinon chaque vérification en laisse une
  // ouverte pour toute la durée de vie de l'application.
  const release = async (resource: Update | null) => {
    if (!resource) return;
    try {
      await resource.close();
    } catch {
      // Ressource déjà libérée ou app en cours de fermeture : sans importance.
    }
  };

  const retryLater = () => {
    if (retryTimer.current) clearTimeout(retryTimer.current);
    retryTimer.current = setTimeout(() => void run(), RETRY_WHEN_BUSY_MS);
  };

  const download = async (pending: Update) => {
    setPhase("downloading");
    setUpdate(pending);
    setProgress(0);
    downloadedBytes.current = 0;
    totalBytes.current = 0;
    await pending.download(
      (event) => {
        switch (event.event) {
          case "Started":
            totalBytes.current = event.data.contentLength ?? 0;
            break;
          case "Progress":
            downloadedBytes.current += event.data.chunkLength;
            if (totalBytes.current > 0) {
              setProgress(
                Math.min(
                  100,
                  Math.round(
                    (downloadedBytes.current / totalBytes.current) * 100,
                  ),
                ),
              );
            }
            break;
          case "Finished":
            setProgress(100);
            break;
        }
      },
      { timeout: DOWNLOAD_TIMEOUT_MS },
    );
  };

  const installAndRelaunch = async (pending: Update) => {
    setPhase("installing");
    setUpdate(pending);
    await pending.install();
    // Sur Windows l'installateur NSIS ferme l'application lui-même : cette
    // ligne n'est atteinte que sur macOS et Linux.
    await relaunch();
  };

  const run = async () => {
    if (busy.current) return;
    busy.current = true;
    let pending: Update | null = null;
    try {
      // Installation portable (Windows) : jamais de mise à jour automatique,
      // et aucun message (l'utilisateur gère son dossier lui-même).
      if (await commands.isPortable()) return;

      // Une tentative précédente a pu télécharger le paquet sans pouvoir
      // l'installer : on repart de là plutôt que de refaire un `check()`.
      pending = downloaded.current ?? (await check());
      if (!pending) return;

      if (!downloaded.current) {
        if (await isBusyElsewhere()) {
          // Rien n'est encore téléchargé : on rend la ressource, la prochaine
          // tentative repartira d'un `check()` neuf.
          await release(pending);
          retryLater();
          return;
        }
        await download(pending);
        downloaded.current = pending;
      }

      // Le téléchargement dure de quelques dizaines de secondes à plusieurs
      // minutes : une dictée a très bien pu démarrer entre-temps, et c'est le
      // redémarrage qui la tuerait. On masque l'écran et on réessaiera plus
      // tard, sans retélécharger.
      if (await isBusyElsewhere()) {
        setUpdate(null);
        retryLater();
        return;
      }

      await installAndRelaunch(pending);
    } catch (e) {
      // Échec silencieux (réseau coupé, aucune release publiée…) : l'app
      // démarre normalement, rien n'est montré à l'utilisateur.
      console.warn("Mise à jour automatique impossible pour le moment :", e);
      downloaded.current = null;
      await release(pending);
      setUpdate(null);
    } finally {
      busy.current = false;
    }
  };

  useEffect(() => {
    void run();
    const timer = setInterval(() => void run(), CHECK_INTERVAL_MS);
    const unlisten = listen("check-for-updates", () => void run());
    return () => {
      clearInterval(timer);
      if (retryTimer.current) clearTimeout(retryTimer.current);
      unlisten.then((fn) => fn());
    };
  }, []);

  if (!update) return null;

  return (
    <div className="fixed inset-0 z-50 flex flex-col items-center justify-center gap-4 bg-background select-none">
      <h2 className="text-base font-semibold">
        {t("updater.title", { version: update.version })}
      </h2>
      <p className="text-sm text-text/70 max-w-md text-center">
        {t("updater.description")}
      </p>
      <div className="w-full max-w-md">
        <ProgressBar
          progress={[{ id: "update", percentage: progress }]}
          size="full"
        />
      </div>
      <p className="text-sm text-text/60 tabular-nums">
        {phase === "installing"
          ? t("updater.installing")
          : t("updater.downloading", { progress })}
      </p>
    </div>
  );
};
