# Installation sur Windows

**Version minimale :** Windows 10 (64 bits). **Fichier :** `OCADE.Dictee_…_x64-setup.exe` — seule version publiée ; elle fonctionne aussi sur Windows ARM, via émulation.

## 1. Installer — avertissement SmartScreen (application non signée)

1. Double-cliquez sur le fichier `.exe`.
2. Écran bleu « Windows a protégé votre ordinateur » : cliquez sur le petit lien **Informations complémentaires**, puis sur **Exécuter quand même**.
3. Suivez les instructions de l'installeur (installation pour l'utilisateur en cours, sans droits administrateur). Laissez **Lancer OCADE Dictée** coché.
   ![SmartScreen](captures/windows-smartscreen.png)

Si un antivirus met le fichier en quarantaine : restaurez-le et ajoutez une exclusion pour « OCADE Dictée ».

**Installation silencieuse (support) :** `"OCADE.Dictee_…_x64-setup.exe" /S` (remplacez `…` par la version, par exemple `1.0.0`)

## 2. Permission microphone

Au premier lancement, si l'application signale l'accès refusé : ouvrez **Paramètres → Confidentialité et sécurité → Microphone**, puis activez **Accès au microphone** et **Autoriser les applications de bureau à accéder au microphone**.

## 3. Téléchargement du modèle

Écran « Préparation d'OCADE Dictée » : ≈ 512 Mo, une seule fois, bouton **Réessayer** en cas de coupure.

## 4. Test de dictée

1. Ouvrez le **Bloc-notes**, cliquez dans la zone de texte.
2. Maintenez **Ctrl + Alt + Espace**, dites « Bonjour, ceci est un test », relâchez.
3. Le texte apparaît, deux sons signalent début et fin.

## 5. Changer le raccourci de dictée

**Paramètres… → Général → Raccourci de dictée.**

- La liste propose quatre raccourcis prêts à l'emploi : **Ctrl + Alt + Espace**, **Ctrl + Maj + Espace**, **Ctrl + Alt + D**, **Ctrl + Maj + D**.
- Pour une autre combinaison, choisissez **Personnalisé…** dans la liste : un champ apparaît sous la liste. Cliquez dessus, puis appuyez sur la combinaison souhaitée et relâchez — le raccourci est enregistré. La touche **Échap** annule la capture.

Certaines combinaisons sont refusées ; la raison s'affiche sous le champ :

- une combinaison **sans touche de modification** (Ctrl, Alt, Maj, touche Windows (affichée « Win » dans l'application)) : elle se déclencherait en pleine frappe ;
- **Maj + une lettre, un chiffre ou une ponctuation** : vous ne pourriez plus taper les majuscules ;
- la touche **Échap** : elle sert déjà à annuler une dictée en cours ;
- les raccourcis **réservés par Windows** (Ctrl + Alt + Suppr, Alt + Tab, Alt + F4, Windows + L, Windows + E…) : le système les intercepte avant l'application.

Quand un raccourci est refusé, **le raccourci précédent reste actif** : la dictée continue de fonctionner.

Après un changement, faites un essai dans une application. Si rien ne se passe, la combinaison est interceptée par Windows avant l'application : choisissez-en une autre.

Évitez aussi les raccourcis courants de vos logiciels (Ctrl + C, Ctrl + V, Ctrl + S…) : OCADE Dictée les intercepterait à leur place.

## 6. Transcrire un fichier audio ou vidéo

L'onglet **Fichier** transcrit aussi un fichier déjà enregistré, sans passer par le micro.

1. Ouvrez l'onglet **Fichier**, puis choisissez un fichier audio ou vidéo (ou une URL de vidéo).
2. Formats acceptés : mp3, mp4, m4a, mov, wav, aac, flac, ogg, oga, opus, aiff, aif, caf, mkv, webm, 3gp, amr, wma, wmv, avi…
3. Pour un premier fichier dans un format qu'elle ne lit pas directement (Opus/WhatsApp, 3GP, WMA, WebM…), l'application télécharge un outil de conversion (45 à 80 Mo ; une connexion Internet est nécessaire à ce moment-là). Ce téléchargement n'a lieu qu'une seule fois.
4. Le texte obtenu peut être exporté au format Markdown.

## 7. Au quotidien

- L'application démarre avec Windows, masquée ; son icône se trouve dans la zone de notification (près de l'horloge, parfois sous la flèche **^**). **Paramètres…** affiche la fenêtre des paramètres.
- Les mises à jour sont automatiques : au lancement, puis toutes les 24 heures, jamais pendant une dictée. Un écran « Mise à jour vers la version X » s'affiche, puis l'application redémarre seule.

## Désinstaller

Ouvrez **Paramètres → Applications → Applications installées → OCADE Dictée → Désinstaller**, puis supprimez `%APPDATA%\com.ocade.handy` (modèle, 512 Mo).
