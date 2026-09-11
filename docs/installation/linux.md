# Installation sur Linux

**Fichier :** `OCADE Dictée_…_amd64.AppImage` — seul format distribué pour Linux (x64, toutes distributions) ; pas de paquet `.deb` ni `.rpm`. Session **X11** recommandée (sous Wayland, l'affichage de la fenêtre d'enregistrement et le collage du texte peuvent être limités).

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

## 5. Transcrire un fichier audio ou vidéo

L'onglet **Fichier** transcrit aussi un fichier déjà enregistré, sans passer par le micro.

1. Ouvrez l'onglet **Fichier**, puis choisissez un fichier audio ou vidéo (ou une URL de vidéo).
2. Formats acceptés : mp3, mp4, m4a, mov, wav, aac, flac, ogg, opus, aiff, caf, mkv, webm, 3gp, amr, wma, wmv, avi…
3. Pour un premier fichier dans un format qu'elle ne lit pas directement (Opus/WhatsApp, 3GP, WMA, WebM…), l'application télécharge un outil de conversion (45 à 80 Mo ; une connexion Internet est nécessaire à ce moment-là). Ce téléchargement n'a lieu qu'une seule fois.
4. Le texte obtenu peut être exporté au format Markdown.

## 6. Au quotidien

- Lancement automatique à l'ouverture de session (entrée XDG autostart) ; icône dans la zone de notification (nécessite une extension AppIndicator sur GNOME).
- Les mises à jour sont automatiques : au lancement, puis toutes les 24 heures, jamais pendant une dictée (l'AppImage se remplace lui-même, sur place). Un écran « Mise à jour vers la version X » s'affiche, puis l'application redémarre seule.

## Désinstaller

Supprimez le fichier `.AppImage`, `~/.config/autostart/*ocade*` et `~/.local/share/com.ocade.handy` (modèle, 512 Mo).
