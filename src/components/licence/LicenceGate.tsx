import React from "react";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";
import HandyTextLogo from "../icons/HandyTextLogo";
import { useLicenceStore } from "@/stores/licenceStore";
import { LicenceKeyForm } from "./LicenceKeyForm";
import { ContactButton } from "./ContactButton";
import { licenceErrorMessage } from "./format";

/**
 * Écran de blocage du mode strict : affiché à la place de l'application tant
 * qu'aucune licence valide n'est active sur cet ordinateur. Même mise en page
 * que l'écran validé lors du travail sur le Microsoft Store (carte centrée à
 * largeur fixe, étiquette, titre, texte, action pleine largeur).
 */
export const LicenceGate: React.FC = () => {
  const { t } = useTranslation();
  const { view, busy, activate, change, refresh } = useLicenceStore();
  if (!view) return null;

  const hasKey = view.key_suffix !== null;
  // Une clé connue mais refusée : on peut d'abord retenter (licence
  // réactivée ou prolongée entre-temps), ou en saisir une autre.
  const reason =
    view.state === "not_activated" ? null : t(`licence.states.${view.state}`);

  const onRecheck = async () => {
    const err = await refresh();
    if (err) toast.error(licenceErrorMessage(t, err));
  };

  return (
    <div className="flex h-screen w-screen select-none items-center justify-center p-6">
      <div className="flex w-full max-w-[28rem] flex-col items-center rounded-2xl border border-mid-gray/20 bg-background-ui/40 px-10 py-10 text-center shadow-2xl">
        <HandyTextLogo width={150} />

        <span className="mt-8 text-[11px] font-semibold uppercase tracking-[0.22em] text-logo-primary">
          {t("licence.gate.badge")}
        </span>
        <h1 className="mt-2 text-2xl font-semibold tracking-tight">
          {t("licence.gate.title")}
        </h1>
        <p className="mt-3 text-sm leading-relaxed text-text/70 text-pretty">
          {t("licence.gate.subtitle")}
        </p>
        {reason && (
          <p className="mt-4 w-full rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs font-medium text-red-400">
            {reason}
          </p>
        )}

        <div className="mt-7 w-full">
          <LicenceKeyForm
            onSubmit={(key) => (hasKey ? change(key) : activate(key))}
            submitLabel={t("licence.actions.activate")}
            busy={busy === "activate" || busy === "change"}
            centered
          />
        </div>

        {hasKey && (
          <button
            type="button"
            disabled={busy !== null}
            onClick={() => void onRecheck()}
            className="mt-4 inline-flex items-center gap-2 text-xs font-medium text-text/60 transition-colors hover:text-text disabled:cursor-default"
          >
            {busy === "refresh" && (
              <Loader2 className="h-3 w-3 animate-spin" aria-hidden="true" />
            )}
            {t("licence.actions.refresh")}
          </button>
        )}

        <p className="mt-6 text-xs leading-relaxed text-text/45 text-pretty">
          {t("licence.gate.contact")}
        </p>
        <div className="mt-2 text-text/70">
          <ContactButton variant="link" />
        </div>
      </div>
    </div>
  );
};
