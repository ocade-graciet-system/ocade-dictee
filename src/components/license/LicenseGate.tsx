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

  // Abonnement déjà souscrit puis arrivé à échéance (le Store a communiqué une
  // date passée) : on le dit ; sinon, premier abonnement, message neutre.
  const lapsed =
    status.expires_at !== null && status.expires_at * 1000 < Date.now();

  return (
    <div className="h-screen w-screen flex items-center justify-center p-8 select-none">
      <div className="w-full max-w-[26rem] flex flex-col items-center text-center">
        <HandyTextLogo width={180} />
        <h1 className="mt-10 text-xl font-semibold">{t("license.title")}</h1>
        <p className="mt-3 text-sm leading-relaxed text-text/70 text-pretty">
          {unreadable
            ? t("license.unavailable")
            : lapsed
              ? t("license.expired")
              : t("license.subtitle")}
        </p>

        <button
          type="button"
          disabled={busy !== null}
          onClick={() => void purchase()}
          className="mt-8 w-56 px-5 py-2.5 rounded-md bg-logo-primary text-white font-medium hover:bg-logo-primary/80 transition-colors disabled:opacity-60 disabled:cursor-default"
        >
          {busy === "purchase"
            ? t("license.purchasing")
            : t("license.subscribe")}
        </button>
        <button
          type="button"
          disabled={busy !== null}
          onClick={() => void recheck()}
          className="mt-3 text-xs text-text/60 underline underline-offset-2 hover:text-text transition-colors disabled:cursor-default"
        >
          {busy === "check" ? t("license.checking") : t("license.recheck")}
        </button>

        <div className="mt-6 min-h-10 flex flex-col items-center gap-2">
          {feedbackText && (
            <p className="text-sm text-red-500 text-pretty">{feedbackText}</p>
          )}
          <p className="text-xs text-text/45 text-pretty">
            {t("license.signedIn")}
          </p>
          {status.detail && (
            <p className="text-xs text-text/35 break-words">{status.detail}</p>
          )}
        </div>
      </div>
    </div>
  );
};

export default LicenseGate;
