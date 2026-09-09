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
// Un échec (réseau coupé, serveur injoignable, release absente…) ne doit pas
// attendre le tick de 24 h : on réessaie dans 30 min, toujours en silence.
const RETRY_AFTER_FAILURE_MS = 30 * 60 * 1000;
// Borne de la requête de vérification. Sans elle, un socket semi-ouvert ne
// rejette jamais : `busy` resterait armé et plus aucune tentative ne repartirait
// de la session.
const CHECK_TIMEOUT_MS = 30 * 1000;
// Borne totale de la requête de téléchargement. Sans elle, un socket
// semi-ouvert ne rejette jamais et l'écran non fermable bloque l'application.
// Large à dessein : le bundle pèse quelques dizaines de Mo.
const DOWNLOAD_TIMEOUT_MS = 15 * 60 * 1000;

// Mo reçus, à une décimale et en typographie française (virgule décimale) :
// l'écran est intégralement en français, la locale système n'a pas à s'y
// substituer.
const megabyteFormatter = new Intl.NumberFormat("fr-FR", {
  minimumFractionDigits: 1,
  maximumFractionDigits: 1,
});

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
  // Mo reçus quand le serveur n'annonce pas de `Content-Length` ; `null` quand
  // la taille totale est connue et que le pourcentage suffit.
  const [receivedMb, setReceivedMb] = useState<number | null>(null);
  const screen = useRef<HTMLDivElement>(null);
  const busy = useRef(false);
  const retryTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const downloadedBytes = useRef(0);
  const totalBytes = useRef(0);
  // Paquet déjà téléchargé mais pas encore installé : gardé d'une tentative à
  // l'autre pour ne jamais retélécharger après un report.
  const downloaded = useRef<Update | null>(null);

  // Une dictée ne doit jamais être interrompue. `isRecording` s'arrête à la fin
  // de l'enregistrement : `isTranscribing` couvre la suite, jusqu'au collage du
  // texte. Une préparation de modèle non plus : elle reprendrait après le
  // redémarrage, mais l'écran de préparation du premier lancement disparaîtrait
  // en plein transfert de 512 Mo. Côté page Fichier, tout est également perdu
  // par un redémarrage : la transcription et le formatage se comptent en
  // minutes, un téléchargement de vidéo repartirait de zéro.
  const isBusyElsewhere = async () => {
    const models = useModelStore.getState();
    const files = useFileTranscriptionStore.getState();
    return (
      (await commands.isRecording()) ||
      (await commands.isTranscribing()) ||
      Object.keys(models.downloadingModels).length > 0 ||
      Object.keys(models.verifyingModels).length > 0 ||
      Object.keys(models.extractingModels).length > 0 ||
      files.status === "processing" ||
      files.formatting ||
      Object.keys(files.videoDownloads).length > 0
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

  const retryLater = (delayMs: number = RETRY_WHEN_BUSY_MS) => {
    if (retryTimer.current) clearTimeout(retryTimer.current);
    retryTimer.current = setTimeout(() => void run(), delayMs);
  };

  const download = async (pending: Update) => {
    setPhase("downloading");
    setUpdate(pending);
    setProgress(0);
    setReceivedMb(null);
    downloadedBytes.current = 0;
    totalBytes.current = 0;
    await pending.download(
      (event) => {
        switch (event.event) {
          case "Started":
            totalBytes.current = event.data.contentLength ?? 0;
            // Sans taille totale, le pourcentage resterait figé à 0 % : on
            // bascule sur le volume reçu.
            setReceivedMb(totalBytes.current > 0 ? null : 0);
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
            } else {
              setReceivedMb(downloadedBytes.current / 1_048_576);
            }
            break;
          case "Finished":
            setProgress(100);
            setReceivedMb(null);
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
      pending =
        downloaded.current ?? (await check({ timeout: CHECK_TIMEOUT_MS }));
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
      retryLater(RETRY_AFTER_FAILURE_MS);
    } finally {
      busy.current = false;
    }
  };

  // L'écran est modal et non fermable : il prend le focus et neutralise le
  // clavier pour que l'interface masquée reste hors d'atteinte (Tab, raccourcis
  // de l'app). Les raccourcis globaux, gérés côté Rust, ne sont pas concernés.
  useEffect(() => {
    if (!update) return;
    screen.current?.focus();
    // `preventDefault` n'annule que l'effet par défaut, pas les écouteurs de
    // l'app : `stopImmediatePropagation` coupe en plus la propagation, la
    // touche ne descend jamais jusqu'à l'interface masquée.
    const swallowKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();
    };
    document.addEventListener("keydown", swallowKey, true);
    return () => document.removeEventListener("keydown", swallowKey, true);
  }, [update]);

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

  // `z-[60]` : au-dessus des `Dialog` (`z-50`), qu'un dialogue resté ouvert ne
  // vienne pas se superposer à une mise à jour en cours.
  return (
    <div
      ref={screen}
      role="dialog"
      aria-modal="true"
      aria-labelledby="updater-title"
      tabIndex={-1}
      className="fixed inset-0 z-[60] flex flex-col items-center justify-center gap-4 bg-background select-none outline-none"
    >
      <h2 id="updater-title" className="text-base font-semibold">
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
          : receivedMb === null
            ? t("updater.downloading", { progress })
            : t("updater.downloadingBytes", {
                megabytes: megabyteFormatter.format(receivedMb),
              })}
      </p>
      {/* Le texte visible change plusieurs fois par seconde : annoncé, il
          noierait la synthèse vocale. Seuls les changements de phase le sont
          ici, via un contenu qui ne dépend que de `phase` — le libellé de
          téléchargement est donc figé à son point de départ, 0 %. */}
      <p className="sr-only" aria-live="polite">
        {`${t("updater.title", { version: update.version })} — ${
          phase === "installing"
            ? t("updater.installing")
            : t("updater.downloading", { progress: 0 })
        }`}
      </p>
    </div>
  );
};
