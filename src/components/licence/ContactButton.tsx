import React from "react";
import { useTranslation } from "react-i18next";
import { Mail } from "lucide-react";
import { toast } from "sonner";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useLicenceStore } from "@/stores/licenceStore";
import { contactMailto } from "./format";

interface ContactButtonProps {
  /** `button` : bouton encadré ; `link` : lien discret (bandeau, écran de blocage). */
  variant?: "button" | "link";
}

/** Ouvre la messagerie sur un e-mail prérempli à destination d'OCADE. */
export const ContactButton: React.FC<ContactButtonProps> = ({
  variant = "button",
}) => {
  const { t } = useTranslation();
  const view = useLicenceStore((s) => s.view);

  const open = async () => {
    const version = await getVersion().catch(() => "");
    try {
      await openUrl(contactMailto(t, view, version));
    } catch (e) {
      console.error("Ouverture de la messagerie impossible :", e);
      toast.error(t("licence.contact.openFailed"));
    }
  };

  if (variant === "link") {
    return (
      <button
        type="button"
        onClick={() => void open()}
        className="inline-flex shrink-0 items-center gap-1.5 rounded-md px-2 py-1 text-xs font-semibold underline-offset-2 hover:underline"
      >
        <Mail className="h-3.5 w-3.5" aria-hidden="true" />
        {t("licence.contact.button")}
      </button>
    );
  }

  return (
    <button
      type="button"
      onClick={() => void open()}
      className="inline-flex items-center gap-2 rounded-lg border border-mid-gray/20 bg-mid-gray/10 px-3 py-1.5 text-sm font-medium transition-colors hover:border-logo-primary"
    >
      <Mail className="h-4 w-4" aria-hidden="true" />
      {t("licence.contact.button")}
    </button>
  );
};
