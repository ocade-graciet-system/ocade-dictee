import React, { useState, useEffect, useRef } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { useTranslation } from "react-i18next";
import { Check, ChevronUp, Download, FileInput, Star, X } from "lucide-react";
import { toast } from "sonner";
import { ProgressBar } from "../shared";
import { useModelStore } from "@/stores/modelStore";
import { commands, type FrModelRank, type ModelInfo } from "@/bindings";

/** Extensions acceptées par « Importer un fichier de modèle… » (même filtre côté Rust). */
const MODEL_FILE_EXTENSIONS = ["bin", "gguf"];

/**
 * Modèle actif + sélecteur (s'ouvre vers le haut). Liste les modèles FR du
 * catalogue — téléchargeables depuis le menu s'ils ne le sont pas encore — et
 * les modèles importés par l'utilisateur depuis un fichier. Le choix est
 * persisté côté Rust (`set_active_model`), donc conservé au prochain
 * lancement. Les modèles sont numérotés selon le classement du banc d’essai
 * français (`FR_MODEL_RANKING`) : 1 = le plus précis, modèle par défaut.
 * Étoile = n° 1.
 */
const ModelSwitcher: React.FC = () => {
  const { t } = useTranslation();
  const {
    models,
    currentModel,
    selectModel,
    downloadModel,
    cancelDownload,
    importModelFile,
    downloadingModels,
    verifyingModels,
    downloadProgress,
  } = useModelStore();
  const [menuOpen, setMenuOpen] = useState(false);
  const [busy, setBusy] = useState<"switch" | "import" | null>(null);
  const [ranking, setRanking] = useState<Record<string, FrModelRank>>({});
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    commands
      .getFrModelRanking()
      .then((list) =>
        setRanking(Object.fromEntries(list.map((r) => [r.id, r]))),
      )
      .catch((e) => console.error("Failed to load model ranking:", e));
  }, []);

  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, []);

  const current = models.find((m) => m.id === currentModel);
  if (!current) return null;

  const activate = async (id: string, name: string) => {
    const ok = await selectModel(id);
    if (!ok) {
      const detail = useModelStore.getState().error;
      toast.error(t("footer.model.activateFailed", { name }), {
        description: detail ?? undefined,
      });
    }
    return ok;
  };

  const pick = async (m: ModelInfo) => {
    setMenuOpen(false);
    if (m.id === currentModel) return;
    setBusy("switch");
    await activate(m.id, m.name);
    setBusy(null);
  };

  // Le menu reste ouvert : la progression s'affiche sur la ligne du modèle.
  // `downloadModel` ne rend la main qu'à la fin du téléchargement (ou à
  // l'échec) ; l'utilisateur peut fermer le menu entre-temps sans l'interrompre.
  const download = async (m: ModelInfo) => {
    const ok = await downloadModel(m.id);
    if (ok) {
      toast.success(t("footer.model.downloadDone", { name: m.name }));
    } else {
      const detail = useModelStore.getState().error;
      toast.error(t("footer.model.downloadFailed", { name: m.name }), {
        description: detail ?? undefined,
      });
    }
  };

  const importFile = async () => {
    setMenuOpen(false);
    let selected: string | null;
    try {
      selected = await openFileDialog({
        multiple: false,
        filters: [
          {
            name: t("footer.model.importDialogFilter"),
            extensions: MODEL_FILE_EXTENSIONS,
          },
        ],
      });
    } catch (e) {
      console.error("Failed to open model file picker:", e);
      toast.error(t("footer.model.importFailed"));
      return;
    }
    if (typeof selected !== "string") return;

    setBusy("import");
    const id = await importModelFile(selected);
    if (id) {
      const imported = useModelStore.getState().models.find((m) => m.id === id);
      const name = imported?.name ?? id;
      if (await activate(id, name)) {
        toast.success(t("footer.model.importDone", { name }));
      }
    } else {
      const detail = useModelStore.getState().error;
      toast.error(t("footer.model.importFailed"), {
        description: detail ?? undefined,
      });
    }
    setBusy(null);
  };

  const star = (
    <span title={t("footer.model.recommended")} className="shrink-0">
      <Star className="w-3 h-3 fill-logo-primary text-logo-primary" />
    </span>
  );

  const importedBadge = (
    <span title={t("footer.model.imported")} className="shrink-0">
      <FileInput className="w-3 h-3 text-text/50" />
    </span>
  );

  const label =
    busy === "switch"
      ? t("footer.model.loading")
      : busy === "import"
        ? t("footer.model.importing")
        : current.name;

  // Rang du classement (« 1. ») et taux d’erreur mesuré, sur toutes les lignes.
  const rankLabel = (m: ModelInfo) => {
    const r = ranking[m.id];
    return (
      <span
        className="w-5 shrink-0 text-right tabular-nums text-text/50"
        title={
          r
            ? t("footer.model.rankHint", {
                rank: r.rank,
                wer: r.wer.toLocaleString("fr-FR"),
              })
            : undefined
        }
      >
        {r ? `${r.rank}.` : ""}
      </span>
    );
  };

  // Colonnes fixes (erreurs, taille, action) : les chiffres restent alignés
  // d’une ligne à l’autre, que le modèle soit installé ou non.
  const werLabel = (m: ModelInfo) => {
    const r = ranking[m.id];
    return (
      <span className="w-12 shrink-0 text-right tabular-nums text-text/50">
        {r
          ? t("footer.model.errorRate", {
              wer: r.wer.toLocaleString("fr-FR", {
                minimumFractionDigits: 1,
                maximumFractionDigits: 1,
              }),
            })
          : ""}
      </span>
    );
  };

  const sizeLabel = (m: ModelInfo) => (
    <span className="w-16 shrink-0 text-right whitespace-nowrap tabular-nums text-text/50">
      {m.is_custom ? "" : t("footer.model.size", { size: m.size_mb })}
    </span>
  );

  const renderRow = (m: ModelInfo) => {
    const isDownloading = m.id in downloadingModels;
    const isVerifying = m.id in verifyingModels;
    const percentage = Math.round(downloadProgress[m.id]?.percentage ?? 0);

    if (m.is_downloaded) {
      return (
        <button
          key={m.id}
          type="button"
          onClick={() => void pick(m)}
          className="w-full flex items-center gap-2 px-3 py-1.5 text-left text-text hover:bg-mid-gray/10"
        >
          <Check
            className={`w-3 h-3 shrink-0 ${m.id === currentModel ? "" : "invisible"}`}
          />
          {rankLabel(m)}
          <span className="flex-1 whitespace-nowrap">{m.name}</span>
          {m.is_recommended && star}
          {m.is_custom && importedBadge}
          {werLabel(m)}
          {sizeLabel(m)}
          <span className="w-28 shrink-0" />
        </button>
      );
    }

    if (isDownloading || isVerifying) {
      return (
        <div
          key={m.id}
          className="w-full flex items-center gap-2 px-3 py-1.5 text-text/70"
        >
          <span className="w-3 shrink-0" />
          {rankLabel(m)}
          <span className="flex-1 whitespace-nowrap">{m.name}</span>
          <ProgressBar
            size="small"
            className="ml-auto"
            progress={[
              { id: m.id, percentage: isVerifying ? 100 : percentage },
            ]}
          />
          <span className="tabular-nums whitespace-nowrap">
            {isVerifying
              ? t("footer.model.verifying")
              : t("footer.model.downloading", { percentage })}
          </span>
          {!isVerifying && (
            <button
              type="button"
              title={t("footer.model.cancel")}
              onClick={() => void cancelDownload(m.id)}
              className="shrink-0 rounded p-0.5 hover:bg-mid-gray/20"
            >
              <X className="w-3 h-3" />
            </button>
          )}
        </div>
      );
    }

    return (
      <div
        key={m.id}
        className="w-full flex items-center gap-2 px-3 py-1.5 text-text/60"
      >
        <span className="w-3 shrink-0" />
        {rankLabel(m)}
        <span className="flex-1 whitespace-nowrap">{m.name}</span>
        {m.is_recommended && star}
        {werLabel(m)}
        {sizeLabel(m)}
        <button
          type="button"
          onClick={() => void download(m)}
          className="w-28 shrink-0 flex items-center justify-end gap-1 rounded px-1.5 py-0.5 text-text hover:bg-mid-gray/20"
        >
          <Download className="w-3 h-3" />
          <span>{t("footer.model.download")}</span>
        </button>
      </div>
    );
  };

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        disabled={busy !== null}
        onClick={() => setMenuOpen((o) => !o)}
        title={t("footer.model.switch")}
        className="flex items-center gap-1 rounded px-1.5 py-0.5 -ml-1.5 hover:bg-mid-gray/10 disabled:hover:bg-transparent disabled:cursor-default"
      >
        {current.is_recommended && star}
        <span>{label}</span>
        <ChevronUp className="w-3 h-3" />
      </button>

      {menuOpen && (
        <div className="absolute bottom-full left-0 mb-1 min-w-[34rem] bg-background border border-mid-gray/80 rounded-md shadow-lg z-50 py-1">
          <div className="px-3 pt-1 pb-1.5 text-text/50">
            {t("footer.model.rankingTitle")}
          </div>
          {models.map(renderRow)}
          <div className="my-1 border-t border-mid-gray/20" />
          <button
            type="button"
            onClick={() => void importFile()}
            title={t("footer.model.importHint")}
            className="w-full flex items-center gap-2 px-3 py-1.5 text-left text-text hover:bg-mid-gray/10"
          >
            <FileInput className="w-3 h-3 shrink-0" />
            <span className="flex-1 whitespace-nowrap">
              {t("footer.model.import")}
            </span>
          </button>
        </div>
      )}
    </div>
  );
};

const Footer: React.FC = () => {
  const [version, setVersion] = useState("");

  useEffect(() => {
    const fetchVersion = async () => {
      try {
        const appVersion = await getVersion();
        setVersion(appVersion);
      } catch (error) {
        console.error("Failed to get app version:", error);
        setVersion("0.1.2");
      }
    };

    fetchVersion();
  }, []);

  return (
    <div className="w-full border-t border-mid-gray/20 pt-3">
      <div className="flex justify-between items-center text-xs px-4 pb-3 text-text/60">
        <div className="flex items-center gap-4">
          <ModelSwitcher />
        </div>

        {/* App version */}
        <div className="flex items-center gap-1">
          {/* eslint-disable-next-line i18next/no-literal-string */}
          <span>v{version}</span>
        </div>
      </div>
    </div>
  );
};

export default Footer;
