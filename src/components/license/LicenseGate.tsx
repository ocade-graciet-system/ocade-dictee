import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleAlert, CircleCheck, Loader2 } from "lucide-react";
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
  // plutôt que de parler d'un abonnement.
  const unreadable = status.detail !== null;
  // Abonnement déjà souscrit puis arrivé à échéance (le Store a communiqué une
  // date passée) : on le dit ; sinon, premier abonnement, message neutre.
  const lapsed =
    status.expires_at !== null && status.expires_at * 1000 < Date.now();

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

  const benefits = [
    t("license.benefits.offline"),
    t("license.benefits.updates"),
    t("license.benefits.renewal"),
  ];

  return (
    <div className="h-screen w-screen flex items-center justify-center p-6 select-none">
      <div className="w-full max-w-[28rem] rounded-2xl border border-mid-gray/20 bg-background-ui/40 shadow-2xl px-10 py-10 flex flex-col items-center text-center">
        <HandyTextLogo width={150} />

        <span className="mt-8 text-[11px] font-semibold uppercase tracking-[0.22em] text-logo-primary">
          {t("license.badge")}
        </span>
        <h1 className="mt-2 text-2xl font-semibold tracking-tight">
          {t("license.title")}
        </h1>
        <p className="mt-3 text-sm leading-relaxed text-text/70 text-pretty">
          {unreadable
            ? t("license.unavailable")
            : lapsed
              ? t("license.expired")
              : t("license.subtitle")}
        </p>

        {!unreadable && (
          <ul className="mt-7 w-full space-y-3 text-left text-sm text-text/85">
            {benefits.map((benefit) => (
              <li key={benefit} className="flex items-start gap-3">
                <CircleCheck className="mt-0.5 h-4 w-4 shrink-0 text-logo-primary" />
                <span className="text-pretty">{benefit}</span>
              </li>
            ))}
          </ul>
        )}

        <button
          type="button"
          disabled={busy !== null}
          onClick={() => void purchase()}
          className="mt-8 w-full inline-flex items-center justify-center gap-2 rounded-lg bg-logo-primary px-5 py-3 text-sm font-semibold text-white shadow-sm transition-colors hover:bg-logo-primary/85 disabled:opacity-60 disabled:cursor-default"
        >
          {busy === "purchase" && (
            <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
          )}
          {busy === "purchase"
            ? t("license.purchasing")
            : t("license.subscribe")}
        </button>
        <button
          type="button"
          disabled={busy !== null}
          onClick={() => void recheck()}
          className="mt-3 inline-flex items-center gap-2 text-xs font-medium text-text/60 transition-colors hover:text-text disabled:cursor-default"
        >
          {busy === "check" && (
            <Loader2 className="h-3 w-3 animate-spin" aria-hidden="true" />
          )}
          {busy === "check" ? t("license.checking") : t("license.recheck")}
        </button>

        {feedbackText && (
          <div
            role="alert"
            className="mt-6 flex w-full items-start gap-2 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2.5 text-left text-xs text-red-400"
          >
            <CircleAlert className="mt-px h-3.5 w-3.5 shrink-0" />
            <span className="text-pretty">{feedbackText}</span>
          </div>
        )}

        <p className="mt-6 text-xs leading-relaxed text-text/45 text-pretty">
          {t("license.signedIn")}
        </p>
        {status.detail && (
          <p className="mt-2 text-[11px] text-text/35 break-words">
            {status.detail}
          </p>
        )}
      </div>
    </div>
  );
};

export default LicenseGate;
