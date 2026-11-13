//! Moteur 2 : bruit large bande par estimation spectrale.
//!
//! Comme l'explique le cahier des charges, le bruit blanc est aléatoire : sa
//! forme d'onde ne se déduit ni de son passé ni du reste du signal, si bien
//! qu'il ne peut pas être annulé par synthèse et soustraction comme une
//! résonance. Ce moteur **atténue** donc un plancher de bruit large bande au
//! lieu de l'annuler : pour chaque case fréquentielle, un gain G(k) ∈
//! [gain_plancher, 1] est calculé à partir du rapport entre la puissance
//! observée et un plancher de bruit estimé, puis lissé dans le temps. La
//! composante retirée vaut (1 − G) × X, ajoutée au parasite spectral comme le
//! fait le moteur de restauration de stem.
//!
//! Le plancher de bruit vient, par ordre de préférence :
//! 1. d'un **profil appris** sur un passage de bruit seul (« Apprendre le
//!    bruit ») : moyenne glissante pendant la capture, gelée dès l'arrêt ;
//! 2. à défaut, d'une **estimation automatique conservatrice** par
//!    statistique du minimum glissant, par nature peu sensible aux
//!    transitoires (une attaque de note, brève, ne fait presque pas monter un
//!    minimum suivi sur plusieurs centaines de millisecondes) — ce qui assure
//!    la protection des attaques et des fins de notes demandée par le cahier
//!    des charges sans mécanisme séparé ; désactivable pour éviter d'apprendre
//!    les notes d'un instrument comme du bruit.

use realfft::num_complex::Complex32;

#[derive(Clone, Copy, Debug)]
pub struct NoiseSettings {
    pub enabled: bool,
    /// Capture en cours : met à jour le profil appris pendant la lecture.
    pub learning: bool,
    /// Estimation automatique conservatrice, utilisée en l'absence de profil appris.
    pub auto_adapt: bool,
    /// Plafond de réduction, en dB.
    pub max_reduction_db: f32,
    /// Lissage du gain dans le temps, en millisecondes.
    pub smoothing_ms: f32,
}

impl Default for NoiseSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            learning: false,
            auto_adapt: true,
            // Un plafond prudent : sur un signal musical dense, toute atténuation
            // touche forcément un peu de signal utile (section 1 du cahier des
            // charges). Mieux vaut un réglage par défaut discret, à monter à
            // l'oreille plutôt que subi.
            max_reduction_db: 12.0,
            smoothing_ms: 150.0,
        }
    }
}

/// État du profil, pour l'affichage.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoiseStatus {
    pub has_profile: bool,
    pub learning: bool,
    /// Le profil actif est l'estimation automatique (pas un profil appris).
    pub using_auto: bool,
    /// Niveau moyen du profil actif, en dB (indicatif).
    pub level_db: f32,
}

/// Rayon, en cases, autour d'une résonance active à exclure du masque de bruit.
const EXCLUDE_BINS: usize = 2;

pub struct NoiseEngine {
    bins: usize,
    bin_hz: f32,
    sample_rate: f32,
    hop: usize,
    settings: NoiseSettings,

    learned_power: Vec<f32>,
    has_learned: bool,
    auto_min: Vec<f32>,
    /// Puissance observée, lissée avant le calcul du gain : la puissance d'une
    /// seule trame est une mesure bien trop bruitée (variance de l'ordre de sa
    /// propre valeur même pour un signal parfaitement stationnaire) pour
    /// décider d'un gain par case sans ce lissage préalable — sans lui, le
    /// gain suit un signal erratique et crée du bruit musical au lieu de le
    /// réduire.
    smoothed_power: Vec<f32>,
    gain: Vec<f32>,
    status: NoiseStatus,
}

/// Constante de temps du lissage de la puissance avant calcul du gain.
const POWER_SMOOTHING_S: f32 = 0.25;

impl NoiseEngine {
    pub fn new(n: usize, hop: usize, sample_rate: f32) -> Self {
        let bins = n / 2 + 1;
        Self {
            bins,
            bin_hz: sample_rate / n as f32,
            sample_rate,
            hop,
            settings: NoiseSettings::default(),
            learned_power: vec![0.0; bins],
            has_learned: false,
            auto_min: vec![f32::MAX; bins],
            smoothed_power: vec![0.0; bins],
            gain: vec![1.0; bins],
            status: NoiseStatus::default(),
        }
    }

    pub fn set_settings(&mut self, s: NoiseSettings) {
        self.settings = s;
    }

    pub fn status(&self) -> NoiseStatus {
        self.status
    }

    pub fn reset_profile(&mut self) {
        self.learned_power.fill(0.0);
        self.has_learned = false;
        self.auto_min.fill(f32::MAX);
    }

    pub fn reset(&mut self) {
        self.reset_profile();
        self.smoothed_power.fill(0.0);
        self.gain.fill(1.0);
        self.status = NoiseStatus::default();
    }

    /// Coefficient d'un lissage exponentiel pour une constante de temps `tau_s`.
    fn coeff(&self, tau_s: f32) -> f32 {
        1.0 - (-(self.hop as f32) / (self.sample_rate * tau_s.max(1e-3))).exp()
    }

    /// Ajoute (1 − G) × X dans `parasite`. `resonance_hz` : fréquences des
    /// résonances actuellement soustraites par le moteur 1, à exclure pour ne
    /// pas les retirer une seconde fois.
    pub fn estimate(&mut self, spectrum: &[Complex32], resonance_hz: &[f32], parasite: &mut [Complex32]) {
        self.status = NoiseStatus::default();
        if !self.settings.enabled {
            return;
        }

        if self.settings.learning {
            let alpha = self.coeff(0.5);
            for (l, x) in self.learned_power.iter_mut().zip(spectrum) {
                let p = x.norm_sqr();
                *l = if self.has_learned { *l + alpha * (p - *l) } else { p };
            }
            self.has_learned = true;
        }

        let floor: &[f32] = if self.has_learned {
            self.status.has_profile = true;
            self.status.learning = self.settings.learning;
            &self.learned_power
        } else if self.settings.auto_adapt {
            let rise = self.coeff(4.0);
            for (m, x) in self.auto_min.iter_mut().zip(spectrum) {
                let p = x.norm_sqr();
                *m = if p < *m { p } else { *m + rise * (p - *m) };
            }
            self.status.has_profile = true;
            self.status.using_auto = true;
            &self.auto_min
        } else {
            // Ni profil appris ni estimation automatique : rien à retirer.
            return;
        };

        let mean: f32 = floor.iter().filter(|v| v.is_finite()).sum::<f32>() / self.bins.max(1) as f32;
        self.status.level_db = 10.0 * (mean + 1e-20).log10();

        let gain_floor = 10f32.powf(-self.settings.max_reduction_db.max(0.0) / 20.0);
        let power_smooth = self.coeff(POWER_SMOOTHING_S);
        let gain_smooth = self.coeff(self.settings.smoothing_ms / 1000.0);

        for k in 0..self.bins {
            let p = spectrum[k].norm_sqr();
            self.smoothed_power[k] += power_smooth * (p - self.smoothed_power[k]);
            let power = self.smoothed_power[k];
            let noise = floor[k].min(power.max(1e-20));
            let ratio = (noise / power.max(1e-20)).min(1.0);
            let target = (1.0 - ratio).sqrt().max(gain_floor);
            self.gain[k] += gain_smooth * (target - self.gain[k]);
        }

        // Les cases déjà traitées par le moteur de résonances ne sont pas
        // retirées une seconde fois.
        for &hz in resonance_hz {
            let center = (hz / self.bin_hz).round() as isize;
            for d in -(EXCLUDE_BINS as isize)..=(EXCLUDE_BINS as isize) {
                let k = center + d;
                if k >= 0 && (k as usize) < self.bins {
                    self.gain[k as usize] = 1.0;
                }
            }
        }

        for k in 0..self.bins {
            if self.gain[k] < 1.0 {
                parasite[k] += spectrum[k] * (1.0 - self.gain[k]);
            }
        }
    }
}
