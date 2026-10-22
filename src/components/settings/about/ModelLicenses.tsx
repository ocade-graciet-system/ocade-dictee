import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { commands, type FrModelRank } from "@/bindings";
import { SettingsGroup } from "../../ui/SettingsGroup";

/** Texte officiel de chaque licence admise (`ALLOWED_MODEL_LICENSES`). */
const LICENSE_TEXT: Record<string, string> = {
  MIT: "https://opensource.org/license/mit",
  "Apache-2.0": "https://www.apache.org/licenses/LICENSE-2.0",
};

/**
 * Modèles de reconnaissance vocale proposés, avec leur auteur et leur
 * licence (MIT ou Apache 2.0) : fournir le texte de la licence est la seule
 * obligation de ces licences. Une source unique, le classement côté Rust.
 */
export const ModelLicenses: React.FC = () => {
  const { t } = useTranslation();
  const [models, setModels] = useState<FrModelRank[]>([]);

  useEffect(() => {
    commands
      .getFrModelRanking()
      .then(setModels)
      .catch((e) => console.error("Failed to load model ranking:", e));
  }, []);

  const open = (url: string) => {
    void openUrl(url).catch((e) => console.error("Failed to open URL:", e));
  };

  return (
    <SettingsGroup title={t("settings.about.models.title")}>
      <div className="px-4 py-3 text-sm space-y-3">
        <p className="text-text/60">{t("settings.about.models.description")}</p>
        <ul className="space-y-1.5">
          {models.map((m) => (
            <li key={m.id} className="flex flex-wrap items-baseline gap-x-2">
              <button
                type="button"
                onClick={() => open(m.source_url)}
                className="font-medium hover:underline"
              >
                {m.name}
              </button>
              <span className="text-text/60">
                {t("settings.about.models.by", { author: m.author })}
              </span>
              <button
                type="button"
                onClick={() => open(LICENSE_TEXT[m.license] ?? m.source_url)}
                className="text-logo-primary hover:underline"
                title={t("settings.about.models.licenseLink")}
              >
                {m.license}
              </button>
            </li>
          ))}
        </ul>
      </div>
    </SettingsGroup>
  );
};
