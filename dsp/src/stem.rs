//! Moteur 3 : restauration d'un stem étouffé par une séparation.
//!
//! Le moteur produit une correction c ajoutée au stem (p = −c, si bien que la
//! chaîne du rack, qui sort −a × p, envoie a × c).
//!
//! - Mode A, avec le mix d'origine sur la sidechain : pour chaque case, m = |S| / |M|
//!   est la part du mix gardée par le séparateur. Au-dessus de la fréquence de
//!   début, la part retenue est remontée au moins jusqu'à la présence du stem
//!   juste sous la bande, et la différence est reprise dans le mix, avec sa phase.
//! - Mode B, sans référence : remontée dynamique des tiers d'octave qui tombent
//!   sous une pente cible prolongeant la bande de référence, et, si demandé,
//!   aigus recréés par copie décalée du médium.

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StemMode {
    #[default]
    Off,
    /// Mode A : mix d'origine sur la sidechain.
    WithMix,
    /// Mode B : sans référence.
    Blind,
}

#[derive(Clone, Copy, Debug)]
pub struct StemSettings {
    pub mode: StemMode,
    /// Fréquence à partir de laquelle on restaure.
    pub start_hz: f32,
    /// Mode A : part de la récupération appliquée (0 à 1).
    pub recovery: f32,
    /// Mode B : pente attendue au-dessus de la bande de référence, en dB/octave.
    pub target_slope_db_oct: f32,
    /// Mode B : remontée maximale, en dB.
    pub max_boost_db: f32,
    /// Mode B : dose d'aigus recréés (0 à 1).
    pub synth: f32,
}

impl Default for StemSettings {
    fn default() -> Self {
        Self {
            mode: StemMode::Off,
            start_hz: 2500.0,
            recovery: 1.0,
            target_slope_db_oct: -4.5,
            max_boost_db: 12.0,
            synth: 0.0,
        }
    }
}

/// État de la sidechain, pour l'affichage.
#[derive(Clone, Copy, Debug, Default)]
pub struct StemStatus {
    /// Niveau de la référence reçue, en dB (trame fenêtrée).
    pub reference_db: f32,
    pub has_reference: bool,
    /// Décalage mesuré : positif si le stem est en retard sur le mix, en échantillons.
    pub lag: i32,
    /// La mesure du décalage est fiable.
    pub lag_reliable: bool,
}

/// Bandes d'un tiers d'octave au-dessus de la fréquence de début (mode B).
const MAX_BANDS: usize = 40;
/// Évaluation du décalage toutes les N trames.
const LAG_EVERY: u32 = 16;

pub struct StemEngine {
    n: usize,
    bin_hz: f32,
    settings: StemSettings,

    // Mode A.
    presence: f32,

    // Mesure du décalage stem/mix (corrélation croisée pondérée par la phase).
    cross: Vec<Complex32>,
    cross_work: Vec<Complex32>,
    lag_buf: Vec<f32>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    ifft_scratch: Vec<Complex32>,
    frames_since_lag: u32,
    status: StemStatus,

    // Mode B.
    band_lo: [usize; MAX_BANDS],
    band_hi: [usize; MAX_BANDS],
    band_boost_db: [f32; MAX_BANDS],
    band_synth: [f32; MAX_BANDS],
    bands: usize,
    bands_start_hz: f32,
    gain: Vec<f32>,
    synth_gain: Vec<f32>,
}

impl StemEngine {
    pub fn new(n: usize, sample_rate: f32) -> Self {
        let bins = n / 2 + 1;
        let ifft = RealFftPlanner::<f32>::new().plan_fft_inverse(n);
        Self {
            n,
            bin_hz: sample_rate / n as f32,
            settings: StemSettings::default(),
            presence: 0.0,
            cross: vec![Complex32::new(0.0, 0.0); bins],
            cross_work: vec![Complex32::new(0.0, 0.0); bins],
            lag_buf: vec![0.0; n],
            ifft_scratch: ifft.make_scratch_vec(),
            ifft,
            frames_since_lag: 0,
            status: StemStatus::default(),
            band_lo: [0; MAX_BANDS],
            band_hi: [0; MAX_BANDS],
            band_boost_db: [0.0; MAX_BANDS],
            band_synth: [0.0; MAX_BANDS],
            bands: 0,
            bands_start_hz: 0.0,
            gain: vec![0.0; bins],
            synth_gain: vec![0.0; bins],
        }
    }

    pub fn set_settings(&mut self, s: StemSettings) {
        self.settings = s;
    }

    pub fn status(&self) -> StemStatus {
        self.status
    }

    pub fn reset(&mut self) {
        self.presence = 0.0;
        self.cross.fill(Complex32::new(0.0, 0.0));
        self.band_boost_db = [0.0; MAX_BANDS];
        self.band_synth = [0.0; MAX_BANDS];
        self.status = StemStatus::default();
    }

    fn start_bin(&self) -> usize {
        let bins = self.n / 2 + 1;
        ((self.settings.start_hz / self.bin_hz).round() as usize).clamp(4, bins - 2)
    }

    /// Ajoute −c dans `parasite`.
    pub fn estimate(&mut self, spectrum: &[Complex32], reference: Option<&[Complex32]>, parasite: &mut [Complex32]) {
        self.track_reference(spectrum, reference);
        match (self.settings.mode, reference) {
            (StemMode::WithMix, Some(mix)) => self.with_mix(spectrum, mix, parasite),
            (StemMode::Blind, _) => self.blind(spectrum, parasite),
            _ => {}
        }
    }

    /// Niveau de la référence et décalage stem/mix.
    fn track_reference(&mut self, spectrum: &[Complex32], reference: Option<&[Complex32]>) {
        let Some(mix) = reference else {
            self.status.has_reference = false;
            return;
        };
        let energy: f32 = mix.iter().map(|m| m.norm_sqr()).sum();
        self.status.reference_db = 10.0 * (energy + 1e-20).log10();
        // Seuil ≈ −90 dBFS pour une trame de 2048 points.
        let floor = 10.0 * ((self.n as f32) * 1e-9).log10();
        self.status.has_reference = self.status.reference_db > floor;
        if !self.status.has_reference {
            return;
        }
        for ((c, s), m) in self.cross.iter_mut().zip(spectrum).zip(mix) {
            *c = *c * 0.9 + s * m.conj() * 0.1;
        }
        self.frames_since_lag += 1;
        if self.frames_since_lag < LAG_EVERY {
            return;
        }
        self.frames_since_lag = 0;
        // Pondération par la phase, limitée aux cases où stem et mix ont
        // vraiment de l'énergie en commun : les cases vidées par le séparateur
        // n'apportent qu'une phase aléatoire.
        let max = self.cross.iter().fold(0.0f32, |m, c| m.max(c.norm()));
        let threshold = (max * 1e-3).max(1e-20);
        for (w, c) in self.cross_work.iter_mut().zip(&self.cross) {
            let mag = c.norm();
            *w = if mag > threshold { c / mag } else { Complex32::new(0.0, 0.0) };
        }
        let last = self.cross_work.len() - 1;
        self.cross_work[0].im = 0.0;
        self.cross_work[last].im = 0.0;
        let _ = self
            .ifft
            .process_with_scratch(&mut self.cross_work, &mut self.lag_buf, &mut self.ifft_scratch);
        let (mut best, mut best_v, mut sum) = (0usize, f32::MIN, 0.0f32);
        for (i, &v) in self.lag_buf.iter().enumerate() {
            sum += v.abs();
            if v > best_v {
                best_v = v;
                best = i;
            }
        }
        let mean = sum / self.lag_buf.len() as f32;
        let lag = if best > self.n / 2 { best as i32 - self.n as i32 } else { best as i32 };
        self.status.lag = lag;
        self.status.lag_reliable = best_v > 8.0 * mean;
    }

    fn with_mix(&mut self, s: &[Complex32], m: &[Complex32], parasite: &mut [Complex32]) {
        let ks = self.start_bin();
        let (lo, hi) = (ks / 2, ks);
        let (mut es, mut em) = (0.0f32, 0.0f32);
        for k in lo..hi {
            es += s[k].norm_sqr();
            em += m[k].norm_sqr();
        }
        let share = if em > 1e-20 { (es / em).sqrt().min(1.0) } else { 0.0 };
        // Attaque immédiate, relâchement sur quelques trames.
        self.presence = if share > self.presence { share } else { self.presence + 0.3 * (share - self.presence) };

        let ramp_end = (ks + ks / 4).max(ks + 1);
        let recovery = self.settings.recovery.clamp(0.0, 1.0);
        for k in ks..parasite.len() {
            let mm = m[k].norm();
            if mm < 1e-12 {
                continue;
            }
            let kept = (s[k].norm() / mm).min(1.0);
            let target = kept.max(self.presence);
            let ramp = ((k - ks) as f32 / (ramp_end - ks) as f32).min(1.0);
            let delta = (target - kept) * recovery * ramp;
            if delta > 0.0 {
                parasite[k] -= m[k] * delta;
            }
        }
    }

    fn prepare_bands(&mut self) {
        let ks = self.start_bin();
        let bins = self.n / 2 + 1;
        let ratio = 2f32.powf(1.0 / 3.0);
        let mut lo = ks;
        let mut b = 0;
        while b < MAX_BANDS && lo < bins - 1 {
            let hi = (((lo as f32) * ratio).ceil() as usize).max(lo + 2).min(bins - 1);
            self.band_lo[b] = lo;
            self.band_hi[b] = hi;
            lo = hi;
            b += 1;
        }
        self.bands = b;
        self.bands_start_hz = self.settings.start_hz;
        self.band_boost_db = [0.0; MAX_BANDS];
        self.band_synth = [0.0; MAX_BANDS];
    }

    fn blind(&mut self, s: &[Complex32], parasite: &mut [Complex32]) {
        if self.bands == 0 || self.bands_start_hz != self.settings.start_hz {
            self.prepare_bands();
        }
        let ks = self.start_bin();
        let mean_db = |a: usize, b: usize| {
            let p: f32 = s[a..b].iter().map(|x| x.norm_sqr()).sum::<f32>() / (b - a).max(1) as f32;
            10.0 * (p + 1e-20).log10()
        };
        let ref_db = mean_db(ks / 2, ks);
        let ref_center = (ks / 2) as f32 * std::f32::consts::SQRT_2;
        // Rien à restaurer dans un passage quasi muet (≈ −100 dBFS).
        let silent = ref_db < 20.0 * (1e-5 * self.n as f32 / std::f32::consts::PI).log10();
        let max_boost = self.settings.max_boost_db.max(0.0);
        let synth = self.settings.synth.clamp(0.0, 1.0);
        let shift = ks / 2;

        for b in 0..self.bands {
            let (lo, hi) = (self.band_lo[b], self.band_hi[b]);
            let center = ((lo * hi) as f32).sqrt();
            let expected = ref_db + self.settings.target_slope_db_oct * (center / ref_center).log2();
            let actual = mean_db(lo, hi);
            let deficit = if silent { 0.0 } else { expected - actual };
            let boost = deficit.clamp(0.0, max_boost);
            self.band_boost_db[b] += 0.3 * (boost - self.band_boost_db[b]);
            // Aigus recréés pour la part du déficit que la remontée ne couvre pas.
            let missing = (deficit - max_boost).max(0.0);
            let synth_target = if synth > 0.0 && missing > 3.0 {
                let src: f32 = s[lo - shift..hi - shift].iter().map(|x| x.norm_sqr()).sum::<f32>()
                    / (hi - lo) as f32;
                let target_pow = 10f32.powf((expected - 3.0) / 10.0);
                synth * (target_pow / (src + 1e-20)).sqrt().min(10.0)
            } else {
                0.0
            };
            self.band_synth[b] += 0.3 * (synth_target - self.band_synth[b]);
        }

        // Gains par case : constants dans chaque tiers d'octave, lissés dans le temps.
        self.gain.fill(0.0);
        self.synth_gain.fill(0.0);
        for b in 0..self.bands {
            let (lo, hi) = (self.band_lo[b], self.band_hi[b]);
            let g = 10f32.powf(self.band_boost_db[b] / 20.0) - 1.0;
            for k in lo..hi {
                self.gain[k] = g;
                self.synth_gain[k] = self.band_synth[b];
            }
        }
        let ramp_end = (ks + ks / 4).max(ks + 1);
        let end = self.band_hi[self.bands.saturating_sub(1)].min(parasite.len());
        for k in ks..end {
            let ramp = ((k - ks) as f32 / (ramp_end - ks) as f32).min(1.0);
            let mut c = s[k] * self.gain[k];
            if self.synth_gain[k] > 0.0 {
                c += s[k - shift] * self.synth_gain[k];
            }
            parasite[k] -= c * ramp;
        }
    }
}
