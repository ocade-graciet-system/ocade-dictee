# Microsoft Store — build, abonnement et soumission

OCADE Dictée existe en deux canaux de distribution, construits depuis le même
code :

|                       | Canal **GitHub** (par défaut)                                 | Canal **Microsoft Store**                                                    |
| --------------------- | ------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| Build                 | `tauri build` (sans feature)                                  | `tauri build --no-bundle --features store` + `scripts/build-msix.ps1`        |
| Format                | installeur NSIS `OCADE.Dictee_x.y.z_x64-setup.exe`, non signé | package `OCADE.Dictee_x.y.z_x64.msix`, signé par le Store                    |
| Licence               | aucune vérification                                           | abonnement annuel (add-on du Store) vérifié au démarrage puis toutes les 6 h |
| Mises à jour          | updater in-app (`latest.json` de la release)                  | par le Store                                                                 |
| Démarrage automatique | clé `Run` du registre (tauri-plugin-autostart)                | `StartupTask` du manifeste MSIX                                              |
| Usage                 | postes de l'éditeur, cas particuliers                         | clients payants                                                              |

Le canal est figé à la compilation par la feature Cargo `store`
(`src-tauri/src/store_channel.rs`). Le front l'obtient par la commande
`get_distribution_channel` et affiche l'écran `LicenseGate` tant que
l'abonnement n'est pas actif.

## 1. Compte et produit dans Partner Center

1. Créer un compte développeur **entreprise** sur
   [Partner Center](https://partner.microsoft.com/dashboard) (99 USD, une fois).
   Les abonnements payants exigent aussi un **profil de paiement et fiscal**
   validé (IBAN, formulaire fiscal).
2. **Apps et jeux → Nouveau produit → Application MSIX ou PWA**, réserver le nom
   « OCADE Dictée ».
3. **Gestion des produits → Identité du produit** : noter les trois valeurs
   `Package/Identity/Name`, `Package/Identity/Publisher` (de la forme
   `CN=…`) et `Package/Properties/PublisherDisplayName`.
4. **Add-ons → Créer un add-on** : type **Abonnement**, période **1 an**,
   prix 50 € (ou le tarif retenu), renouvellement automatique. Une fois
   créé, noter son **Store ID** (`9N…`).

## 2. Variables du dépôt GitHub

Dans _Settings → Secrets and variables → Actions → Variables_ :

| Variable                             | Valeur                                |
| ------------------------------------ | ------------------------------------- |
| `OCADE_STORE_IDENTITY_NAME`          | `Package/Identity/Name`               |
| `OCADE_STORE_PUBLISHER`              | `Package/Identity/Publisher` (`CN=…`) |
| `OCADE_STORE_PUBLISHER_DISPLAY_NAME` | `PublisherDisplayName`                |
| `OCADE_STORE_SUBSCRIPTION_ID`        | Store ID de l'add-on abonnement       |

Tant que ces variables sont absentes, le job MSIX de `release.yml` se termine
sans produire de package et **ne bloque pas** la release GitHub.

Dès qu'elles sont définies, chaque release publie en plus l'asset
`OCADE.Dictee_x.y.z_x64.msix`, non signé, prêt pour Partner Center.

## 3. Soumission

1. Partner Center → le produit → **Nouvelle soumission**.
2. **Packages** : glisser le `.msix` de la release. Le Store vérifie l'identité
   (nom, éditeur, version) et le signe lui-même.
3. **Propriétés** : catégorie Productivité ; déclarer l'accès au microphone ;
   URL de la politique de confidentialité (obligatoire, l'app utilise le
   micro) ; âge.
4. **Tarification** : app gratuite + add-on abonnement (déjà créé). L'app
   bloque l'usage sans abonnement, c'est l'écran `LicenseGate`.
5. **Notes de certification** : préciser que l'application est une app de
   bureau « full trust » (raccourci clavier global, collage dans l'application
   active), qu'elle télécharge un modèle de 512 Mo au premier lancement, et
   comment tester l'abonnement (compte de test ou ajout de l'add-on).
6. La certification prend en général 1 à 3 jours ouvrés.

À chaque release suivante, répéter la soumission avec le nouveau `.msix`
(numéro de version strictement supérieur, le script ajoute `.0` en 4ᵉ
composant). L'automatisation via l'API Partner Center est possible dans un
second temps.

## 4. Test local du package

Sur un poste avec le SDK Windows (Build Tools 2022, workload C++) :

```powershell
bun run tauri build --no-bundle --features store
.\scripts\build-msix.ps1 -SelfSign
```

Le script crée un certificat de test, signe le package et affiche les deux
commandes à lancer en administrateur (import du certificat dans
_Trusted People_, puis `Add-AppxPackage`). L'application installée ainsi
n'est pas associée au Store : l'écran d'abonnement affiche « La licence n'a
pas pu être vérifiée » avec la cause technique, ce qui est attendu. Le
parcours d'achat complet ne se teste qu'avec un package passé par le Store
(soumission en « visibilité privée » ou mode test de Partner Center).

## 5. Ce que fait l'application côté Store

- `store_channel::license_status()` lit `StoreContext.GetAppLicenseAsync`,
  puis cherche dans `AddOnLicenses` une entrée dont la clé commence par le
  Store ID de l'abonnement et dont `IsActive` est vrai. Windows met la licence
  en cache : l'appel répond aussi hors ligne.
- `purchase_subscription()` rattache le `StoreContext` à la fenêtre principale
  (`IInitializeWithWindow`) puis appelle `RequestPurchaseAsync(storeId)` ; la
  fenêtre d'achat est celle du Store.
- Un abonnement arrivé à échéance bloque l'application au prochain démarrage
  ou à la relecture suivante (toutes les 6 h), avec le bouton « S'abonner ».
- Le démarrage automatique passe par `StartupTask` (`TaskId=OcadeDicteeStartup`
  dans `src-tauri/msix/AppxManifest.xml`) ; l'utilisateur peut toujours le
  couper dans _Paramètres → Applications → Démarrage_.

## 6. Points d'attention

- **WebView2** : le package ne l'installe pas. Il est présent sur Windows 11
  et sur tout Windows 10 à jour (Edge) ; c'est le cas des postes clients.
- **Données** : constaté sur Windows 11 avec le package de test, l'app
  packagée lit et écrit le même dossier `%APPDATA%\com.ocade.handy` que
  l'installeur GitHub (réglages, modèles, journaux). Un client qui passe de
  l'un à l'autre retrouve donc son modèle et ses réglages.
- **Les deux canaux ne doivent pas cohabiter** sur un même poste : l'instance
  unique est partagée (même identifiant), la seconde lancée se contente de
  réveiller la première. Désinstaller la version GitHub avant d'installer
  celle du Store, et inversement.
