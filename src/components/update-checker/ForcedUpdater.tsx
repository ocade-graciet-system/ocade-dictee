import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { listen } from "@tauri-apps/api/event";
import { ProgressBar } from "../shared";
import { useModelStore } from "../../stores/modelStore";
import { commands } from "@/bindings";

const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000; // toutes les 24 h (issue #6)
const RETRY_WHEN_BUSY_MS = 5 * 60 * 1000; // dictée ou téléchargement en cours → réessai dans 5 min

// Mise à jour forcée : aucune question, aucun report. Montée à la racine de
// l'app pour couvrir aussi les écrans de premier lancement.
export const ForcedUpdater: React.FC = () => {
  const { t } = useTranslation();
  const [update, setUpdate] = useState<Update | null>(null);
  const [progress, setProgress] = useState(0);
  const busy = useRef(false);
  const retryTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const downloadedBytes = useRef(0);
  const totalBytes = useRef(0);

  // Une dictée ne doit jamais être interrompue. Un téléchargement de modèle non
  // plus : il reprendrait après le redémarrage, mais l'écran de préparation du
  // premier lancement disparaîtrait en plein transfert de 512 Mo.
  const isBusyElsewhere = async () =>
    (await commands.isRecording()) ||
    Object.keys(useModelStore.getState().downloadingModels).length > 0;

  const install = async (pending: Update) => {
    setUpdate(pending);
    setProgress(0);
    downloadedBytes.current = 0;
    totalBytes.current = 0;
    await pending.downloadAndInstall((event) => {
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
    });
    // Sur Windows l'installateur NSIS ferme l'application lui-même : cette
    // ligne n'est atteinte que sur macOS et Linux.
    await relaunch();
  };

  const run = async () => {
    if (busy.current) return;
    busy.current = true;
    try {
      // Installation portable (Windows) : jamais de mise à jour automatique,
      // et aucun message (l'utilisateur gère son dossier lui-même).
      if (await commands.isPortable()) return;
      const pending = await check();
      if (!pending) return;
      if (await isBusyElsewhere()) {
        if (retryTimer.current) clearTimeout(retryTimer.current);
        retryTimer.current = setTimeout(() => void run(), RETRY_WHEN_BUSY_MS);
        return;
      }
      await install(pending);
    } catch (e) {
      // Échec silencieux (réseau coupé, aucune release publiée…) : l'app
      // démarre normalement, rien n'est montré à l'utilisateur.
      console.warn("Mise à jour automatique impossible pour le moment :", e);
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
        {progress < 100
          ? t("updater.downloading", { progress })
          : t("updater.installing")}
      </p>
    </div>
  );
};
