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

Les binaires sont non signés tant que la variable de dépôt `SIGN_BINARIES` n'est pas à `true` (voir issue #13).

**Secrets et variables du workflow de release :**

- **Obligatoires** : le secret `TAURI_SIGNING_PRIVATE_KEY` (clé privée minisign de l'updater ; sa clé publique est `plugins.updater.pubkey` dans `src-tauri/tauri.conf.json` ; sans ce secret, `createUpdaterArtifacts: true` fait échouer le build) et, si elle est protégée par un mot de passe, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Sans eux, le build échoue.
- **Optionnels** (signature OS des binaires, pas encore activée) : la variable de dépôt `SIGN_BINARIES=true`, plus les secrets `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD` (macOS) et `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_TENANT_ID` (Windows).

## Licence

MIT — voir [LICENSE](LICENSE). Composants tiers : Whisper (OpenAI), ggml/whisper.cpp, modèle `whisper-large-v3-french-distil-dec2` (bofenghuang, Hugging Face), yt-dlp.
