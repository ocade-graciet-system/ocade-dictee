# Contribuer à OCADE Dictée

OCADE Dictée est un fork de [Handy](https://github.com/cjpais/Handy) (MIT) adapté en outil de dictée **clé en main** pour des utilisateurs non techniques. Le cadrage complet est dans l'épic [#1](https://github.com/ocade-graciet-system/ocade-dictee/issues/1).

## Règles

1. **Une PR par issue**, branche `feat/<sujet>` ou `fix/<sujet>` créée depuis `v1-cle-en-main` (ou `main` après la 1.0).
2. Avant d'ouvrir la PR : `bun run format && bun run lint && bun run build && (cd src-tauri && cargo test) && bun run check:translations`.
3. **Tout ce qui n'est pas réglable n'apparaît nulle part dans l'outil** (voir principe n° 1 de l'épic). Ne pas réintroduire de réglage sans décision dans une issue.
4. **Chaque comportement doit fonctionner sur macOS, Windows et Linux.** Ce qui n'a pu être testé que sur un OS est listé dans la PR, section « À vérifier en recette ».
5. Interface en français. Ajouter les clés dans `src/i18n/locales/fr/translation.json` — et, tant que d'autres locales existent dans `src/i18n/locales/`, dans `en/translation.json` aussi (le contrôle `check:translations` compare chaque locale à `en`).

## Démarrer

```bash
git clone https://github.com/ocade-graciet-system/ocade-dictee.git
cd ocade-dictee
bun install
bun tauri dev
```

Prérequis par OS : voir [BUILD.md](BUILD.md).
