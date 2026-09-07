#!/usr/bin/env bash
#
# OCADE — Signature auto-signée STABLE pour des autorisations macOS persistantes.
#
# Problème résolu : en signature ad-hoc, l'autorisation d'accessibilité (clavier)
# est liée à l'empreinte (cdhash) du build. Chaque recompilation change cette
# empreinte → macOS « réoublie » l'autorisation.  Avec un certificat stable,
# l'autorisation est liée au CERTIFICAT, pas au build → elle persiste à travers
# tous les rebuilds (tant que l'app est resignée avec ce même certificat).
#
# À lancer UNE fois. Ré-exécutable sans risque (idempotent).
# Nécessite ton mot de passe de session (import trousseau / codesign).
#
set -euo pipefail

CERT_NAME="OCADE Signing"
APP="/Applications/OCADE.app"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENTITLEMENTS="$REPO_ROOT/src-tauri/Entitlements.plist"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"
BUNDLE_ID="com.ocade.handy"

echo "==> 0/4  Arrêt d'OCADE si en cours"
pkill -9 -f "OCADE.app/Contents/MacOS/handy" 2>/dev/null && echo "         arrêtée." || echo "         pas en cours."

echo "==> 1/4  Certificat de signature « $CERT_NAME »"
if security find-certificate -c "$CERT_NAME" >/dev/null 2>&1; then
  echo "         déjà présent — on le réutilise."
else
  TMP="$(mktemp -d)"
  cat > "$TMP/openssl.cnf" <<'CNF'
[req]
distinguished_name = dn
x509_extensions = v3
prompt = no
[dn]
CN = OCADE Signing
[v3]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
CNF
  openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -keyout "$TMP/key.pem" -out "$TMP/cert.pem" -config "$TMP/openssl.cnf" >/dev/null 2>&1
  openssl pkcs12 -export -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
    -out "$TMP/id.p12" -passout pass: >/dev/null 2>&1
  # -A : autorise les apps (dont codesign) à utiliser la clé sans invite répétée.
  security import "$TMP/id.p12" -k "$KEYCHAIN" -P "" -A
  rm -rf "$TMP"
  echo "         créé et importé dans ton trousseau de session."
fi

echo "==> 2/4  Signature de $APP avec l'identité stable"
codesign --force --deep --options runtime \
  --entitlements "$ENTITLEMENTS" \
  --sign "$CERT_NAME" "$APP"

echo "==> 3/4  Vérification de la signature"
codesign -dv "$APP" 2>&1 | grep -E 'Authority=|Identifier=' || true
codesign --verify --deep --strict "$APP" && echo "         signature valide."

echo "==> 4/4  Réinitialisation de l'autorisation d'accessibilité (sera redemandée proprement)"
tccutil reset Accessibility "$BUNDLE_ID" || true

echo
echo "✅ TERMINÉ."
echo "   1) Relance OCADE depuis /Applications."
echo "   2) Accorde l'accessibilité UNE fois (Réglages Système → Accessibilité → OCADE → ON)."
echo "   Elle persistera désormais à travers les rebuilds."
