# Installation sur Linux

**Fichier :** `OCADE.Dictee_…_amd64.AppImage` — seul format distribué pour Linux (x64, toutes distributions) ; pas de paquet `.deb` ni `.rpm`. Session **X11** recommandée (sous Wayland, l'affichage de la fenêtre d'enregistrement et le collage du texte peuvent être limités) ; 8 Go de mémoire recommandés au minimum pour le résumé.

## 1. Installer

1. Déplacez le fichier dans un dossier permanent où vous avez le droit d'écriture, par exemple `~/Applications/` (les mises à jour remplacent ce fichier automatiquement, au même endroit).
2. Rendez-le exécutable : clic droit → **Propriétés → Permissions → Autoriser l'exécution**, ou dans un terminal :
   ```
   chmod +x ~/Applications/OCADE*Dictée*.AppImage
   ```
3. Double-cliquez pour lancer. Si rien ne se passe (Ubuntu 22.04+), installez `libfuse2` : `sudo apt install libfuse2`.

## 2. Permission microphone

Selon l'environnement (GNOME, KDE), une demande d'accès au micro peut apparaître : acceptez-la. Sinon, vérifiez dans **Paramètres → Son → Entrée** que le micro est actif.

## 3. Téléchargement du modèle

Écran « Préparation d'OCADE Dictée » : ≈ 512 Mo, une seule fois, bouton **Réessayer** en cas de coupure.

## 4. Test de dictée

1. Ouvrez un éditeur de texte, cliquez dans la zone de texte.
2. Maintenez **Ctrl + Alt + Espace**, dites « Bonjour, ceci est un test », relâchez.
3. Le texte apparaît, deux sons signalent début et fin.

## 5. Changer le raccourci de dictée

**Paramètres… → Général → Raccourci de dictée.**

Sous Linux, la liste propose quatre raccourcis prêts à l'emploi : **Ctrl + Alt + Espace**, **Ctrl + Maj + Espace**, **Ctrl + Alt + D**, **Ctrl + Maj + D** ; la capture d'une combinaison personnalisée n'est pas disponible sur Linux dans cette version (l'entrée **Personnalisé…** affiche alors « La capture d'un raccourci n'est pas disponible sur cet ordinateur »).

Après un changement, faites un essai dans une application. Si rien ne se passe, la combinaison est déjà utilisée par votre environnement de bureau ou par un autre logiciel : choisissez-en une autre.

## 6. Transcrire un fichier audio ou vidéo

L'onglet **Fichier** transcrit aussi un fichier déjà enregistré, sans passer par le micro.

1. Ouvrez l'onglet **Fichier**, puis choisissez un fichier audio ou vidéo (ou une URL de vidéo).
2. Formats acceptés : mp3, mp4, m4a, mov, wav, aac, flac, ogg, oga, opus, aiff, aif, caf, mkv, webm, 3gp, amr, wma, wmv, avi…
3. Pour un premier fichier dans un format qu'elle ne lit pas directement (Opus/WhatsApp, 3GP, WMA, WebM…), l'application télécharge un outil de conversion (45 à 80 Mo ; une connexion Internet est nécessaire à ce moment-là). Ce téléchargement n'a lieu qu'une seule fois.
4. Le texte obtenu peut être exporté au format Markdown.

## 7. Résumer un enregistrement

Après une transcription (ou en rouvrant une entrée de l'historique), le bouton **Résumer** de l'onglet **Fichier** produit un compte-rendu en français : un titre, un résumé, les points clés et, s'il y en a, les décisions et actions.

1. Au premier usage, l'application télécharge une seule fois le moteur de résumé et son modèle (environ 2 Go ; une connexion Internet est nécessaire à ce moment-là). Le téléchargement peut être annulé et reprend là où il s'était arrêté. Un antivirus peut analyser ces fichiers pendant quelques minutes.
2. Le compte-rendu est calculé entièrement sur votre ordinateur : rien n'est envoyé sur Internet. Temps indicatif pour 1 h d'enregistrement : 3 à 5 minutes sur un ordinateur récent, sans carte graphique dédiée ; un enregistrement de 2 h peut prendre 10 à 15 minutes.
3. **Copier le compte-rendu** le place dans le presse-papier avec sa mise en forme (LibreOffice, Thunderbird…) ; **Enregistrer sous…** produit un fichier Markdown contenant la transcription puis le compte-rendu.
4. Le compte-rendu est conservé avec l'entrée dans l'historique de l'onglet **Fichier** ; **Résumer** permet de le recalculer.

## 8. Au quotidien

- Lancement automatique à l'ouverture de session (une entrée de démarrage automatique est créée dans `~/.config/autostart/`) ; icône dans la zone de notification (sur GNOME, installez l'extension GNOME Shell « AppIndicator » depuis les extensions GNOME).
- Les mises à jour sont automatiques : au lancement, puis toutes les 24 heures, jamais pendant une dictée (l'AppImage se remplace lui-même, sur place). Un écran « Mise à jour vers la version X » s'affiche, puis l'application redémarre seule.

## Désinstaller

1. Quittez l'application (icône → **Quitter**).
2. Supprimez le fichier `.AppImage`, l'entrée de démarrage `~/.config/autostart/OCADE*Dictée*` et le dossier `~/.local/share/com.ocade.handy` (modèles et outils : 512 Mo, jusqu'à 2,7 Go si le résumé a été utilisé).
