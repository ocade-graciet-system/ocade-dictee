import React from "react";
import { useTranslation } from "react-i18next";
import { CircleAlert, Info, TriangleAlert } from "lucide-react";
import { useLicenceStore } from "@/stores/licenceStore";
import { bannerFor } from "./format";
import { ContactButton } from "./ContactButton";

const TONES = {
  info: {
    box: "border-logo-primary/30 bg-logo-primary/10 text-text",
    icon: Info,
    iconClass: "text-logo-primary",
  },
  warning: {
    box: "border-amber-400/40 bg-amber-400/10 text-text",
    icon: TriangleAlert,
    iconClass: "text-amber-400",
  },
  error: {
    box: "border-red-500/40 bg-red-500/10 text-text",
    icon: CircleAlert,
    iconClass: "text-red-400",
  },
} as const;

interface LicenceBannerProps {
  /** Ouvre l'onglet Licence. Absent quand on y est déjà. */
  onOpen?: () => void;
}

/** Rappel de licence en haut de l'application (échéance, hors ligne, refus). */
export const LicenceBanner: React.FC<LicenceBannerProps> = ({ onOpen }) => {
  const { t } = useTranslation();
  const view = useLicenceStore((s) => s.view);
  if (!view) return null;
  const content = bannerFor(t, view);
  if (!content) return null;
  const tone = TONES[content.tone];
  const Icon = tone.icon;

  return (
    <div
      role={content.tone === "info" ? "status" : "alert"}
      className={`flex items-center gap-3 border-b px-4 py-2 text-sm ${tone.box}`}
    >
      <Icon
        className={`h-4 w-4 shrink-0 ${tone.iconClass}`}
        aria-hidden="true"
      />
      <p className="flex-1 text-pretty">{content.text}</p>
      {onOpen && (
        <button
          type="button"
          onClick={onOpen}
          className="shrink-0 rounded-md px-2 py-1 text-xs font-semibold underline-offset-2 hover:underline"
        >
          {t("licence.actions.open")}
        </button>
      )}
      <ContactButton variant="link" />
    </div>
  );
};
