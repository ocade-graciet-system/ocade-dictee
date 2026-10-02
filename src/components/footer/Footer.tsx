import React, { useState, useEffect, useRef } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { useTranslation } from "react-i18next";
import { Check, ChevronUp, Star } from "lucide-react";
import { useModelStore } from "@/stores/modelStore";

/**
 * Modèle actif + sélecteur (s'ouvre vers le haut). Ne liste que les modèles
 * téléchargés ; le choix est persisté côté Rust (`set_active_model`), donc
 * conservé au prochain lancement. Étoile = modèle recommandé (comparatif FR).
 */
const ModelSwitcher: React.FC = () => {
  const { t } = useTranslation();
  const { models, currentModel, selectModel } = useModelStore();
  const [open, setOpen] = useState(false);
  const [switching, setSwitching] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, []);

  const downloaded = models.filter((m) => m.is_downloaded);
  const current = models.find((m) => m.id === currentModel);
  if (!current) return null;

  const pick = async (id: string) => {
    setOpen(false);
    if (id === currentModel) return;
    setSwitching(true);
    await selectModel(id);
    setSwitching(false);
  };

  const star = (
    <span title={t("footer.model.recommended")} className="shrink-0">
      <Star className="w-3 h-3 fill-logo-primary text-logo-primary" />
    </span>
  );

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        disabled={downloaded.length < 2 || switching}
        onClick={() => setOpen((o) => !o)}
        title={t("footer.model.switch")}
        className="flex items-center gap-1 rounded px-1.5 py-0.5 -ml-1.5 hover:bg-mid-gray/10 disabled:hover:bg-transparent disabled:cursor-default"
      >
        {current.is_recommended && star}
        <span>{switching ? t("footer.model.loading") : current.name}</span>
        {downloaded.length > 1 && <ChevronUp className="w-3 h-3" />}
      </button>

      {open && (
        <div className="absolute bottom-full left-0 mb-1 min-w-56 bg-background border border-mid-gray/80 rounded-md shadow-lg z-50 py-1">
          {downloaded.map((m) => (
            <button
              key={m.id}
              type="button"
              onClick={() => pick(m.id)}
              className="w-full flex items-center gap-2 px-3 py-1.5 text-left text-text hover:bg-mid-gray/10"
            >
              <Check
                className={`w-3 h-3 shrink-0 ${m.id === currentModel ? "" : "invisible"}`}
              />
              <span className="flex-1 whitespace-nowrap">{m.name}</span>
              {m.is_recommended && star}
            </button>
          ))}
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
