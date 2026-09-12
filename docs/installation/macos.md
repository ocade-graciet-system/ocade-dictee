# Installation sur macOS

**Version minimale :** macOS 10.15. **Fichier :** `OCADE.Dictee_…_aarch64.dmg` (Mac Apple Silicon : M1, M2, M3, M4…) ou `…_x64.dmg` (Mac Intel). Pour savoir lequel utiliser : menu → **À propos de ce Mac** → ligne **Puce** (Apple M…) ou **Processeur** (Intel).

## 1. Installer

1. Double-cliquez sur le fichier `.dmg`.
2. Glissez **OCADE Dictée** dans le dossier **Applications**.
3. Éjectez le disque « OCADE Dictée » (clic droit → **Éjecter**) et supprimez le `.dmg`.

## 2. Premier lancement — avertissement de sécurité (application non signée)

1. Ouvrez **Applications** et double-cliquez sur **OCADE Dictée**.
2. macOS affiche « Apple n'a pas pu confirmer… ». Cliquez sur **OK** (l'application ne démarre pas encore).
3. Ouvrez **Réglages Système → Confidentialité et sécurité** (sur macOS 10.15 à 12 : **Préférences Système → Sécurité et confidentialité**), descendez tout en bas : à côté du message concernant OCADE Dictée, cliquez sur **Ouvrir quand même**, puis sur **Ouvrir** dans la confirmation.
4. Si le bouton n'apparaît pas : relancez l'application depuis **Applications** (le bouton apparaît après une première tentative d'ouverture). En dernier recours, le support dispose d'une manipulation dédiée (voir la fiche support).

![Ouvrir quand même](captures/macos-ouvrir-quand-meme.png)

Cet avertissement ne s'affiche qu'à la toute première ouverture de l'application : il ne réapparaît pas après une mise à jour automatique.

## 3. Permissions

L'application demande deux autorisations. Sans elles, la dictée ne fonctionne pas.

1. **Microphone** : cliquez sur **Autoriser** dans la fenêtre système.
2. **Accessibilité** (nécessaire pour coller le texte) : la fenêtre **Réglages Système → Confidentialité et sécurité → Accessibilité** s'affiche ; activez l'interrupteur **OCADE Dictée** (mot de passe de session demandé).
3. Revenez dans OCADE Dictée : l'écran continue automatiquement.

## 4. Téléchargement du modèle

L'écran « Préparation d'OCADE Dictée » télécharge le modèle français (≈ 512 Mo). Connexion Internet nécessaire **une seule fois**. En cas de coupure : bouton **Réessayer**.

## 5. Test de dictée

1. Ouvrez **Notes** ou **TextEdit**, placez le curseur dans le texte.
2. Maintenez le raccourci (par défaut **⌃ ⌥ Espace**), dites « Bonjour, ceci est un test », relâchez.
3. Le texte apparaît. Deux sons signalent le début et la fin.

## 6. Transcrire un fichier audio ou vidéo

L'onglet **Fichier** transcrit aussi un fichier déjà enregistré, sans passer par le micro.

1. Ouvrez l'onglet **Fichier**, puis choisissez un fichier audio ou vidéo (ou une URL de vidéo).
2. Formats acceptés : mp3, mp4, m4a, mov, wav, aac, flac, ogg, oga, opus, aiff, aif, caf, mkv, webm, 3gp, amr, wma, wmv, avi…
3. Pour un premier fichier dans un format qu'elle ne lit pas directement (Opus/WhatsApp, 3GP, WMA, WebM…), l'application télécharge un outil de conversion (45 à 80 Mo ; une connexion Internet est nécessaire à ce moment-là). Ce téléchargement n'a lieu qu'une seule fois.
4. Le texte obtenu peut être exporté au format Markdown.

## 7. Au quotidien

- L'application se lance à l'ouverture de session, masquée ; son icône se trouve dans la barre de menus (en haut à droite). **Paramètres…** affiche la fenêtre des paramètres.
- Les mises à jour sont automatiques : au lancement, puis toutes les 24 heures, jamais pendant une dictée. Un écran « Mise à jour vers la version X » s'affiche, puis l'application redémarre seule.

## Désinstaller

1. Quittez l'application (icône de la barre de menus → **Quitter**).
2. Supprimez `/Applications/OCADE Dictée.app`.
3. Supprimez le dossier de données (modèle, 512 Mo) : dans le Finder, menu **Aller → Aller au dossier…**, collez `~/Library/Application Support/com.ocade.handy`, puis supprimez ce dossier.
