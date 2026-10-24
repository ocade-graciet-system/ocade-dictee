# Compte rendu de réunion : faisabilité avec le modèle local

Banc d'essai du 24 octobre 2026, sur le PC de développement Windows (Intel Core i9-14900HX, 64 Go, CPU seul).

Question posée : le futur bouton « Compte rendu de réunion » peut-il appliquer la **charte éditoriale**, la **charte graphique** et le **skill** ajoutés dans l'onglet Réunion, avec le modèle que l'application propose ? Ce modèle est Ministral 3 3B Instruct Q4_K_M, servi par `llama-server` avec un contexte de 12 288 tokens (`src-tauri/src/summary/`).

## Verdict

| Élément | Faisable ? | Comment |
|---|---|---|
| **Skill** (structure du compte rendu) | **Oui** | Le modèle suit la structure demandée dans 16 cas sur 16 : sections dans l'ordre, titres exacts, tableau `Qui \| Quoi \| Échéance` bien formé. Un contrôle tiré des titres du skill, avec une relance, couvre le cas restant (sortie coupée). |
| **Charte graphique** | **Oui, sans le modèle** | Le modèle rend un Markdown propre : aucun bloc de code ni ligne `---` sur 24 sorties. L'application applique elle-même le logo, les couleurs, la police et le pied de page au document. |
| **Charte éditoriale** | **En partie** | Le modèle suit les règles simples : vouvoiement, pas de « on », heures « 14 h 30 », montants « 2 400 € HT ». Il suit mal le lexique interdit (7 à 10 mots interdits par compte rendu, contre 9 sans charte), le format des dates (0 %), la longueur des phrases et le gras des noms. **Les règles mécaniques doivent être appliquées par le code** : la démonstration atteint 0 mot interdit et 100 % de dates conformes. |

## Points d'attention mesurés

- **Années inventées.** Quand la charte demande d'écrire l'année, le modèle invente « 2023 » dans 8 essais sur 12. Donner la date de l'enregistrement dans le prompt supprime le problème. Le défaut existe déjà dans les notes intermédiaires du compte-rendu actuel de l'onglet Fichier.
- **Fidélité.** Environ un essai sur deux attribue une action à la mauvaise personne, et le tableau ne garde que 3 à 5 actions sur 8. Ce défaut existe aussi sans charte. Le résultat doit être présenté comme **un brouillon à relire**.
- **Exemples du skill recopiés.** Une phrase d'exemple du skill a été reprise telle quelle dans 14 sorties sur 17, dont 2 décisions inventées. Les exemples d'un skill doivent être à trous (`<décision>`), pas réalistes.
- **Taille.** Une charte de 2 500 mots ne fait pas mieux qu'une charte de 650 mots, mais elle allonge le calcul. Près du seuil de tranche, elle sature le contexte : sortie coupée, et 17 min avec la relance. Au-delà de 12 288 tokens, le serveur répond HTTP 400 `exceed_context_size_error`. Le code actuel affiche alors un échec de démarrage du moteur, ce qui est trompeur.
- **Temps de calcul sur ce CPU :**
  - environ 2 min 30 à 3 min pour 2 000 mots de transcription (~15 min de réunion), charte et skill compris ;
  - environ 12 min pour 7 000 mots.
  - Augmenter le contexte n'est pas recommandé : il faut +0,4 à +2 Go de mémoire, et la génération devient plus lente.

## Ce que le bouton « Compte rendu de réunion » devra faire

1. **Plafonds.** Charte d'environ 1 000 tokens (≈ 650 mots) et skill d'environ 800 tokens (≈ 500 mots). L'onglet Réunion avertit déjà au-delà.
2. **Tranche dynamique.** En passe directe, la taille de la transcription est réduite de celle de la charte et du skill. Une longue réunion passe par les notes, qui restent neutres. La charte et le skill ne servent qu'à la synthèse finale.
3. **Placement dans le prompt :**
   - charte dans le message système ;
   - skill avant la transcription ou les notes ;
   - **date de l'enregistrement** dans tous les prompts ;
   - **rappel condensé des règles** après la transcription.
4. **Post-traitement par le code** pour les règles mécaniques. Il demande une charte qui expose ses règles vérifiables, par exemple sous forme de tableau « Interdit | À écrire » :
   - lexique ;
   - dates (jour et année calculés depuis la date d'enregistrement ; une incohérence est signalée, pas corrigée en silence) ;
   - heures, montants, gras des participants.
5. **Contrôle de structure dérivé du skill**, avec une seule relance, sur le modèle de `check_template`.
6. **Erreur de dépassement de contexte** traduite en message clair, ou évitée grâce aux points 1 et 2.
7. **Rendu du document** (Word ou PDF) avec la charte graphique : logo, couleurs, police, pied de page.

## Défauts du compte-rendu actuel (onglet Fichier) relevés au passage

- Les notes de la première tranche sont coupées à 1 400 tokens dans 2 essais sur 2 : 64 à 72 puces au lieu des 25 demandées.
- Les années sont inventées dans les notes, faute de date de référence dans le prompt.
- Un dépassement de contexte est présenté comme un échec de démarrage du moteur.

## Limites du banc

- 2 essais par scénario, à la température 0,2.
- Transcriptions fictives, plus propres qu'une vraie sortie de reconnaissance vocale.
- Contrôles automatiques par expressions régulières ; fidélité relevée à la main par un seul lecteur.
- Non testés : d'autres modèles, un cache KV quantifié, la charte dans les notes.
