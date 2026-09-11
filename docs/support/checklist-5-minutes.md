# Checklist support — 5 minutes après installation

- [ ] L'app est lancée : icône visible (barre de menus macOS / zone de notification Windows-Linux)
- [ ] Réglages → Général : le raccourci choisi est affiché ; le bon micro est sélectionné
- [ ] Dictée dans l'application métier du client (pas seulement le Bloc-notes) : texte collé, presse-papier de l'utilisateur intact (copier un mot avant la dictée, le recoller après : il est toujours là)
- [ ] Les deux sons (début/fin) sont audibles sur le périphérique de sortie utilisé
- [ ] Session fermée puis rouverte : l'app est relancée automatiquement, masquée
- [ ] À propos : la version affichée est la dernière release
- [ ] macOS : Microphone et Accessibilité sont cochés pour OCADE Dictée dans Réglages Système
- [ ] Noter le modèle de machine, l'OS et la version dans la fiche client

## Mode débogage (diagnostic avancé)

À utiliser si un problème résiste à la checklist ci-dessus.

1. Quittez complètement l'application (icône de la barre → **Quitter**).
2. Relancez-la depuis un terminal, avec l'option `--debug` :
   - macOS : `"/Applications/OCADE Dictée.app/Contents/MacOS/handy" --debug`
   - Windows : `"%LOCALAPPDATA%\OCADE Dictée\handy.exe" --debug`
   - Linux : `./OCADE*.AppImage --debug`
3. Un onglet **Débogage** apparaît dans l'application, avec l'emplacement des journaux.
4. `--debug` n'a aucun effet si une instance de l'application tourne déjà : vérifiez à l'étape 1 qu'elle est bien fermée avant de relancer.
