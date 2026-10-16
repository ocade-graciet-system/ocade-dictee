import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import HandyTextLogo from "../icons/HandyTextLogo";
import { commands, type StoreLicenseStatus } from "@/bindings";

interface LicenseGateProps {
  /** Dernier état lu par App (déjà inactif), pour afficher le bon message sans re-lire. */
  initialStatus: StoreLicenseStatus;
  /** Appelé dès qu'un abonnement actif est constaté : App reprend son flux normal. */
  onLicensed: () => void;
}

type Feedback =
  null | "not_purchased" | "purchase_failed" | "still_inactive" | "unavailable";

/**
 * Écran bloquant du build Microsoft Store : tant que l'abonnement annuel
 * (add-on du Store) n'est pas actif, rien d'autre n'est accessible. Le bouton
 * « S'abonner » ouvre la fenêtre d'achat du Store, rattachée à la fenêtre de
 * l'application ; « Vérifier à nouveau » relit la licence (cas d'un achat fait
 * sur le site du Store ou d'un changement de compte).
 */
const LicenseGate: React.FC<LicenseGateProps> = ({
  initialStatus,
  onLicensed,
}) => {
  const { t } = useTranslation();
  const [status, setStatus] = useState<StoreLicenseStatus>(initialStatus);
  const [busy, setBusy] = useState<"check" | "purchase" | null>(null);
  const [feedback, setFeedback] = useState<Feedback>(null);

  useEffect(() => {
    if (status.active) onLicensed();
  }, [status.active, onLicensed]);

  const recheck = async (): Promise<boolean> => {
    setBusy("check");
    setFeedback(null);
    const result = await commands.getStoreLicense();
    setBusy(null);
    if (result.status !== "ok") {
      setFeedback("unavailable");
      return false;
    }
    setStatus(result.data);
    if (result.data.active) return true;
    setFeedback(result.data.detail ? "unavailable" : "still_inactive");
    return false;
  };

  const purchase = async () => {
    setBusy("purchase");
    setFeedback(null);
    const result = await commands.purchaseStoreSubscription();
    if (result.status !== "ok") {
      setBusy(null);
      setFeedback("purchase_failed");
      return;
    }
    switch (result.data) {
      case "succeeded":
      case "already_purchased":
        // La licence est mise à jour par le Store : on la relit pour passer.
        await recheck();
        return;
      case "not_purchased":
        setBusy(null);
        setFeedback("not_purchased");
        return;
      case "failed":
        setBusy(null);
        setFeedback("purchase_failed");
        return;
    }
  };

  // Licence illisible (installation hors Store, API absente) : on l'explique
  // plutôt que de parler d'un abonnement expiré.
  const unreadable = status.detail !== null;

  const feedbackText =
    feedback === "not_purchased"
      ? t("license.purchaseNotCompleted")
      : feedback === "purchase_failed"
        ? t("license.purchaseFailed")
        : feedback === "still_inactive"
          ? t("license.stillInactive")
          : feedback === "unavailable"
            ? t("license.unavailable")
            : null;

  return (
    <div className="h-screen w-screen flex flex-col items-center justify-center gap-6 p-8 text-center select-none">
      <HandyTextLogo width={200} />
      <h1 className="text-lg font-semibold">{t("license.title")}</h1>
      <p className="text-text/70 max-w-md">
        {unreadable ? t("license.unavailable") : t("license.subtitle")}
      </p>
      {!unreadable && (
        <p className="text-sm text-text/60 max-w-md">{t("license.expired")}</p>
      )}
      <div className="flex flex-col items-center gap-3 w-full max-w-md">
        <button
          type="button"
          disabled={busy !== null}
          onClick={() => void purchase()}
          className="px-5 py-2 rounded bg-logo-primary text-white hover:bg-logo-primary/80 transition-colors disabled:opacity-60 disabled:cursor-default"
        >
          {busy === "purchase"
            ? t("license.purchasing")
            : t("license.subscribe")}
        </button>
        <button
          type="button"
          disabled={busy !== null}
          onClick={() => void recheck()}
          className="text-xs text-text/60 underline hover:text-text transition-colors disabled:cursor-default"
        >
          {busy === "check" ? t("license.checking") : t("license.recheck")}
        </button>
        <p className="text-xs text-text/50 max-w-md">{t("license.signedIn")}</p>
        {feedbackText && (
          <p className="text-sm text-red-500 max-w-md">{feedbackText}</p>
        )}
        {status.detail && (
          <p className="text-xs text-text/40 break-words max-w-md">
            {status.detail}
          </p>
        )}
      </div>
    </div>
  );
};

export default LicenseGate;
