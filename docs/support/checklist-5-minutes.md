# Checklist support — 5 minutes après installation

- [ ] L'app est lancée : icône visible (barre de menus macOS / zone de notification Windows-Linux)
- [ ] Paramètres… → Général : le raccourci choisi est affiché ; le bon micro est sélectionné
- [ ] Dictée dans l'application métier du client (pas seulement le Bloc-notes) : texte collé, presse-papier de l'utilisateur intact (copier un mot avant la dictée, le recoller après : il est toujours là)
- [ ] Les deux sons (début/fin) sont audibles sur le périphérique de sortie utilisé
- [ ] Session fermée puis rouverte : l'app est relancée automatiquement, masquée
- [ ] À propos : la version affichée est la dernière release
- [ ] macOS : Microphone et Accessibilité sont cochés pour OCADE Dictée dans Réglages Système (Préférences Système avant macOS 13 ; si Accessibilité semble cochée sans effet après une mise à jour, décochez/recochez ou retirez (−) et rajoutez (+))
- [ ] Noter le modèle de machine, l'OS et la version dans la fiche client

## Raccourci refusé, ou dictée sans réaction après un changement

1. **Message « Ce raccourci n'est pas accepté : … »** sous le champ : la combinaison est interdite (sans touche de modification, Maj + un caractère, Échap, ou réservée par le système). Le raccourci précédent est resté actif, la dictée fonctionne toujours. Proposez au client une autre combinaison, ou l'un des quatre raccourcis de la liste.
2. **Le raccourci est bien affiché mais la dictée ne démarre pas** : la combinaison est captée par le système ou par un autre logiciel avant OCADE Dictée (raccourci personnalisé du système, logiciel de visioconférence, gestionnaire de fenêtres). Repassez sur le premier raccourci de la liste pour vérifier que la dictée répond, puis choisissez une autre combinaison personnalisée.
3. **Message « le système a refusé de l'enregistrer »** : la combinaison est déjà prise par une autre application lancée. Fermez-la, ou choisissez une autre combinaison.
4. **Aucun raccourci ne fonctionne** : vérifiez l'autorisation Accessibilité (macOS, voir la case ci-dessus) puis relancez l'application.

## Mode débogage (diagnostic avancé)

À utiliser si un problème résiste à la checklist ci-dessus.

1. Quittez complètement l'application (icône de la barre → **Quitter**).
2. Relancez-la depuis un terminal, avec l'option `--debug` :
   - macOS : `"/Applications/OCADE Dictée.app/Contents/MacOS/handy" --debug`
   - Windows : `"%LOCALAPPDATA%\OCADE Dictée\handy.exe" --debug`
   - Linux : `./OCADE*.AppImage --debug`
3. Un onglet **Débogage** apparaît dans l'application, avec l'emplacement des journaux.
4. `--debug` n'a aucun effet si une instance de l'application tourne déjà : vérifiez à l'étape 1 qu'elle est bien fermée avant de relancer.

## macOS — si « Ouvrir quand même » n'apparaît jamais (support uniquement)

Réservé au support, jamais demandé au client. Dans le **Terminal** de la machine concernée, exécutez la ligne suivante puis relancez l'application :

```
xattr -dr com.apple.quarantine "/Applications/OCADE Dictée.app"
```

Elle retire l'attribut de quarantaine posé par le navigateur au téléchargement ; les mises à jour automatiques n'en posent pas.
