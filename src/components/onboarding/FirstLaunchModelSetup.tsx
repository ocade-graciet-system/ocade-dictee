import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import HandyTextLogo from "../icons/HandyTextLogo";
import { ProgressBar } from "../shared";
import { useModelStore } from "../../stores/modelStore";

/** Identifiant du modèle unique — égal à DEFAULT_FR_MODEL_ID (src-tauri/src/managers/model.rs). */
export const DEFAULT_FR_MODEL_ID = "whisper-distil-fr-dec2-q5_0";

interface FirstLaunchModelSetupProps {
  onReady: () => void;
  /**
   * `onboarding_completed` était déjà vrai au démarrage (utilisateur connu, lu
   * par `App.checkOnboardingStatus`). Un échec d'activation lui laisse alors
   * la possibilité d'entrer quand même dans l'application, qu'il utilisait
   * déjà : pour un nouvel utilisateur il n'y a rien derrière cet écran.
   */
  isReturningUser: boolean;
}

/** Ce qui a échoué : le message affiché et les issues proposées en dépendent. */
type FailureKind = "download" | "activation";

// Premier lancement (issue #2) : télécharge le modèle FR sans aucun choix,
// affiche la progression, permet de réessayer, puis l'active et continue.
// Si le fichier est déjà sur le disque, l'écran enchaîne directement sur
// l'activation — aucun clic n'est jamais demandé en dehors d'un échec.
const FirstLaunchModelSetup: React.FC<FirstLaunchModelSetupProps> = ({
  onReady,
  isReturningUser,
}) => {
  const { t } = useTranslation();
  const {
    models,
    error,
    initialize,
    downloadModel,
    selectModel,
    downloadProgress,
    downloadStats,
    downloadingModels,
    verifyingModels,
  } = useModelStore();
  const [failed, setFailed] = useState<FailureKind | null>(null);
  const started = useRef(false);
  const finishing = useRef(false);

  const model = models.find((m) => m.id === DEFAULT_FR_MODEL_ID);
  const percentage = downloadProgress[DEFAULT_FR_MODEL_ID]?.percentage ?? 0;
  const speed = downloadStats[DEFAULT_FR_MODEL_ID]?.speed ?? 0;
  const downloading = DEFAULT_FR_MODEL_ID in downloadingModels;
  const verifying = DEFAULT_FR_MODEL_ID in verifyingModels;
  // Le fichier est sur le disque : il ne reste que le chargement du modèle,
  // qui prend quelques secondes — on ne réaffiche pas une progression à 0 %.
  const activating =
    model?.is_downloaded === true && !downloading && !verifying;

  const startDownload = async () => {
    setFailed(null);
    const ok = await downloadModel(DEFAULT_FR_MODEL_ID);
    if (!ok) setFailed("download");
  };

  // Active le modèle puis rend la main à App. Un échec ici est anormal (le
  // fichier est présent et vérifié) : la cause technique est affichée sous le
  // message, c'est la seule information exploitable.
  const activate = async () => {
    const ok = await selectModel(DEFAULT_FR_MODEL_ID);
    if (ok) {
      onReady();
    } else {
      finishing.current = false;
      setFailed("activation");
    }
  };

  useEffect(() => {
    void initialize();
  }, [initialize]);

  // Le store consigne aussi les échecs remontés par event (model-download-failed).
  // `??` : ne jamais écraser un message déjà affiché, notamment celui de l'activation.
  useEffect(() => {
    if (error && !downloading) setFailed((previous) => previous ?? "download");
  }, [error, downloading]);

  // Un seul démarrage par montage : `startDownload` est volontairement hors
  // dépendances (il est recréé à chaque rendu), le garde `started` suffit.
  useEffect(() => {
    if (!model || started.current) return;
    started.current = true;
    if (!model.is_downloaded) void startDownload();
  }, [model]);

  // Même remarque que pour `startDownload` : `activate` est recréé à chaque
  // rendu, l'ajouter aux dépendances relancerait l'effet en boucle. Le garde
  // `finishing` assure déjà l'unicité de l'appel.
  useEffect(() => {
    if (!activating || finishing.current) return;
    finishing.current = true;
    void activate();
  }, [activating]);

  // Ne re-télécharger (512 Mo) que si le fichier manque encore : après un échec
  // d'activation, seule l'activation est retentée.
  const handleRetry = () => {
    setFailed(null);
    if (model?.is_downloaded) {
      finishing.current = true;
      void activate();
    } else {
      void startDownload();
    }
  };

  return (
    <div className="h-screen w-screen flex flex-col items-center justify-center gap-6 p-8 text-center select-none">
      <HandyTextLogo width={200} />
      <h1 className="text-lg font-semibold">
        {t("onboarding.firstLaunch.title")}
      </h1>
      <p className="text-text/70 max-w-md">
        {t("onboarding.firstLaunch.subtitle")}
      </p>
      {failed ? (
        <div className="w-full max-w-md space-y-3">
          <p className="text-sm text-red-500">
            {failed === "activation"
              ? t("onboarding.firstLaunch.selectFailed")
              : t("onboarding.firstLaunch.failed")}
          </p>
          {/* Cause réelle, en anglais et technique : elle ne remplace pas le
              message ci-dessus mais reste la seule piste exploitable. */}
          {error && <p className="text-xs text-text/50 break-words">{error}</p>}
          <div className="flex flex-col items-center gap-2">
            <button
              type="button"
              onClick={handleRetry}
              className="px-4 py-2 rounded bg-logo-primary text-white hover:bg-logo-primary/80 transition-colors"
            >
              {t("onboarding.firstLaunch.retry")}
            </button>
            {failed === "activation" && isReturningUser && (
              <button
                type="button"
                onClick={onReady}
                className="text-xs text-text/60 underline hover:text-text transition-colors"
              >
                {t("onboarding.firstLaunch.continueAnyway")}
              </button>
            )}
          </div>
        </div>
      ) : (
        <div className="w-full max-w-md space-y-2">
          <ProgressBar
            progress={[
              {
                id: DEFAULT_FR_MODEL_ID,
                percentage: activating ? 100 : percentage,
              },
            ]}
            size="full"
          />
          <p className="text-sm text-text/60 tabular-nums">
            {verifying
              ? t("onboarding.firstLaunch.verifying")
              : activating
                ? t("onboarding.firstLaunch.activating")
                : t("onboarding.firstLaunch.downloading", {
                    // `percentage` est un f64 brut côté Rust : sans arrondi
                    // l'écran affiche « Téléchargement… 42.99479765414 % ».
                    percentage: Math.round(percentage),
                    speed: speed.toFixed(1),
                  })}
          </p>
        </div>
      )}
    </div>
  );
};

export default FirstLaunchModelSetup;
