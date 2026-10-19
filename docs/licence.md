# Licence dans OCADE Dictée

L'application vérifie sa licence auprès du serveur de licences
`ocade-graciet-system/ocade-dictee-licences`, qui fait référence pour tout
comportement (contrat de l'API, codes d'erreur, format du jeton).

## Fonctionnement

- L'utilisateur saisit sa clé dans l'onglet **Licence** (`OCAD-XXXX-XXXX-XXXX-XXXX`).
  La clé est validée **localement** (alphabet de 32 symboles, contrôle Luhn mod 32)
  avant tout appel réseau, en Rust (`src-tauri/src/licence/key.rs`) comme en
  TypeScript (`src/components/licence/format.ts`), avec les mêmes vecteurs de test
  que le serveur.
- Le serveur renvoie un **jeton signé Ed25519**, valable 14 jours au plus et jamais
  au-delà de l'échéance de la licence. L'application le vérifie **hors ligne** avec
  les clés publiques embarquées (`src-tauri/src/licence/config.rs`), puis le
  renouvelle au démarrage et toutes les 24 h (toutes les 30 minutes après un échec,
  ou plus tard si le serveur répond `Retry-After`).
- Seuls `revoked`, `archived`, `expired`, `unknown_key` et `not_activated` effacent
  le jeton. Une erreur réseau, une réponse illisible, `rate_limited` ou
  `internal_error` ne touchent ni au jeton ni à la clé.
- `refresh` ne réactive jamais un poste libéré par un opérateur : l'utilisateur
  ressaisit sa clé.
- **Mode** lu sur `/mode` : `soft` par défaut (rien n'est bloqué), `strict` (la
  dictée et la transcription de fichiers sont refusées sans licence valide, la
  fenêtre s'affiche sur l'écran de blocage).
- Rappels dans l'application : bandeau à 30 jours de l'échéance, alerte à 7 jours,
  alerte hors ligne. Une licence sans échéance n'a ni jours restants ni bandeau.
- État local : `licence.json` dans le dossier de données (clé, jeton, dernier refus,
  dernier mode, plus grande heure vue, qui ne recule jamais).

## Clé publique de production

`PRODUCTION_KEYS` est **vide** tant que la clé `k1` n'est pas générée côté serveur
(`npm run gen:key`, base64 standard). Une version publiée sans elle reste
utilisable en mode souple, mais **aucune clé ne peut s'activer**. La PR qui
l'ajoute doit être fusionnée avant de compter sur l'activation.

## Essai en développement

`tauri dev` ignore la licence, sauf si `OCADE_LICENCE_URL` désigne un serveur local.
Ces variables n'ont **aucun effet en release** :

| Variable                | Rôle                                                      |
| ----------------------- | --------------------------------------------------------- |
| `OCADE_LICENCE_URL`     | Base de l'API, par exemple `http://127.0.0.1:8787/api/v1` |
| `OCADE_LICENCE_PUBKEYS` | Clés publiques de test, `k1:<base64>[,k2:<base64>]`       |
| `OCADE_LICENCE_BYPASS`  | `1` pour ignorer la licence même avec une URL             |

Deux serveurs possibles :

- **Le vrai serveur** (`ocade-dictee-licences`, PostgreSQL 16 requis) : voir son
  README ; c'est la référence.
- **Le serveur de test** sans base, `node scripts/licence-dev-server.mjs` : il
  affiche les deux variables à définir et propose des routes `/_dev/*` pour créer
  une clé, la révoquer, l'expirer ou passer en mode strict (exemples en tête du
  script).

Sur le poste Windows de développement :

```powershell
$env:OCADE_LICENCE_URL = "http://127.0.0.1:8787/api/v1"
$env:OCADE_LICENCE_PUBKEYS = "k1:<clé publique affichée par le serveur>"
.\dev-windows.ps1
```
