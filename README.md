# VST_SoundScalpel-

**SoundScalpel** est un effet VST3 pour Windows (Ableton Live 12) écrit en Rust avec [nih-plug](https://github.com/robbert-vdh/nih-plug). Il nettoie un son par **soustraction en inversion de phase** plutôt que par égalisation :

- **Résonances et sifflements** : les pics agressifs sont recréés puis annulés, sans creuser la bande de fréquences.
- **Bruit large bande** : atténuation d'un plancher de bruit, appris sur un passage de bruit seul ou estimé automatiquement.
- **Restauration de stem** : redonne leurs aigus aux stems étouffés par une séparation, avec le mix d'origine (mode A) ou sans référence (mode B).

## Documentation

- [Guide utilisateur](docs/Guide_utilisateur.md) : installation, mise en place dans Live, réglages, dépannage.
- [Cahier des charges](docs/Cahier_des_charges_SoundScalpel.docx) : spécification fonctionnelle et technique.

## Construire

```powershell
cargo xtask bundle soundscalpel --release
```

Le plugin est produit dans `target\bundled\soundscalpel.vst3`, à copier dans `C:\Program Files\Common Files\VST3\`.

## Licence

SoundScalpel est publié sous [GPL-3.0-or-later](LICENSE). Ce choix n'est pas arbitraire : `vst3-sys`, qui relie nih-plug au SDK VST3 de Steinberg, est lui-même sous GPLv3 — un binaire compilé qui l'embarque est une œuvre dérivée, et sa distribution doit respecter la GPLv3. Voir [docs/THIRD-PARTY-LICENSES.md](docs/THIRD-PARTY-LICENSES.md) pour la licence de chaque dépendance.
