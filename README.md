# OCADE Dictée

Dictée vocale **locale** et **clé en main** : maintenez un raccourci, parlez, relâchez — le texte est collé dans l'application active. Aucune donnée ne quitte l'ordinateur. Français uniquement. macOS, Windows et Linux.

> Fork de [Handy](https://github.com/cjpais/Handy) (MIT, © CJ Pais), simplifié pour des utilisateurs non techniques. Cadrage : [épic #1](https://github.com/ocade-graciet-system/ocade-dictee/issues/1).

## Installation

Téléchargez la **[dernière release](https://github.com/ocade-graciet-system/ocade-dictee/releases/latest)** — un fichier par machine :

| Votre machine                                              | Fichier            |
| ---------------------------------------------------------- | ------------------ |
| **Windows** (64 bits, y compris Windows ARM via émulation) | `…_x64-setup.exe`  |
| **Mac Apple Silicon** (M1 et suivants)                     | `…_aarch64.dmg`    |
| **Mac Intel** (modèles ≈ avant 2020)                       | `…_x64.dmg`        |
| **Linux** (x64, toutes distributions)                      | `…_amd64.AppImage` |

Les procédures détaillées par système (permissions, avertissements de sécurité, désinstallation) sont dans [`docs/installation/`](docs/installation/).

Au premier lancement, l'application télécharge le modèle de reconnaissance français (≈ 512 Mo) puis est prête. Les mises à jour sont automatiques.

## Installation chez le client

Procédures pas à pas pour accompagner l'installation sur le poste d'un client (avertissements de sécurité, permissions, désinstallation) et checklist de vérification après installation : [`docs/installation/README.md`](docs/installation/README.md).

## Utilisation

1. Choisissez votre raccourci dans **Général** (4 propositions, identiques sur les 3 systèmes).
2. Maintenez-le, parlez, relâchez : le texte apparaît dans l'application active.
3. Onglet **Fichier** : transcription d'un fichier audio/vidéo ou d'une URL, export Markdown.

## Développement

```bash
git clone https://github.com/ocade-graciet-system/ocade-dictee.git
cd ocade-dictee
bun install
bun tauri dev
```

Prérequis par OS : [BUILD.md](BUILD.md). Règles de contribution : [CONTRIBUTING.md](CONTRIBUTING.md).

### Releases

Un tag `vX.Y.Z` poussé sur ce dépôt déclenche `.github/workflows/release.yml` : build des 4 artefacts, `latest.json` signé pour l'updater, release GitHub (pré-release si le tag contient un suffixe, ex. `v1.0.0-rc.1` — elle n'est jamais servie comme « latest » à l'updater).

```bash
# 1. Mettre à jour la version dans src-tauri/tauri.conf.json, package.json, src-tauri/Cargo.toml
# 2. git tag -a v1.0.0 -m "1.0.0" && git push origin v1.0.0
```

Les binaires macOS sont signés avec une identité stable dès que la variable de dépôt `SIGN_MACOS` est à `true` (voir issue #29). Windows reste non signé tant que `SIGN_BINARIES` n'est pas à `true` (voir issue #13).

**Secrets et variables du workflow de release :**

- **Obligatoires** : le secret `TAURI_SIGNING_PRIVATE_KEY` (clé privée minisign de l'updater ; sa clé publique est `plugins.updater.pubkey` dans `src-tauri/tauri.conf.json` ; sans ce secret, `createUpdaterArtifacts: true` fait échouer le build) et, si elle est protégée par un mot de passe, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Sans eux, le build échoue.
- **macOS** (optionnel) : la variable de dépôt `SIGN_MACOS=true`, plus les secrets `APPLE_CERTIFICATE` (certificat p12 encodé en base64), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` (le CN du certificat) et `KEYCHAIN_PASSWORD`. Identité stable (Developer ID **ou** certificat auto-signé) : contrairement à une signature ad hoc, l'empreinte ne change pas à chaque build, donc l'autorisation Accessibilité (TCC) accordée par l'utilisateur survit aux mises à jour. Pas de notarisation : l'avertissement Gatekeeper reste affiché à la toute première installation (voir [`docs/installation/macos.md`](docs/installation/macos.md)).

  Génération d'un certificat auto-signé (OpenSSL) :

  ```bash
  cat > codesign.cnf <<'EOF'
  [req]
  distinguished_name = dn
  x509_extensions = ext
  prompt = no
  [dn]
  CN = OCADE Fusion Code Signing
  [ext]
  keyUsage = critical, digitalSignature
  extendedKeyUsage = critical, codeSigning
  basicConstraints = critical, CA:false
  EOF
  openssl req -x509 -newkey rsa:2048 -keyout key.pem -out cert.pem -days 3650 -nodes -config codesign.cnf -extensions ext
  openssl pkcs12 -export -inkey key.pem -in cert.pem -out certificate.p12 -passout pass:"<mot de passe>"
  base64 -i certificate.p12 -o certificate.p12.base64   # macOS ; sous Linux : base64 -w0 certificate.p12 > certificate.p12.base64
  ```

  Le contenu de `certificate.p12.base64` va dans le secret `APPLE_CERTIFICATE`, le mot de passe choisi dans `APPLE_CERTIFICATE_PASSWORD`, et le CN (`OCADE Fusion Code Signing`) dans `APPLE_SIGNING_IDENTITY`.

- **Windows** (optionnel, réservé à Azure Trusted Signing et à une future notarisation) : la variable de dépôt `SIGN_BINARIES=true`, plus les secrets `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_TENANT_ID`.

## Licence

MIT — voir [LICENSE](LICENSE). Composants tiers : Whisper (OpenAI), ggml/whisper.cpp, modèle `whisper-large-v3-french-distil-dec2` (bofenghuang, Hugging Face), yt-dlp.
