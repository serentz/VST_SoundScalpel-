//! Moteur 1 : résonances et sifflements par synthèse et soustraction.
//!
//! À chaque trame :
//! 1. détection des pics étroits et proéminents par rapport au voisinage ;
//! 2. fréquence fine par la différence de phase entre deux trames (vocodeur de
//!    phase), affinée en minimisant l'erreur d'ajustement ;
//! 3. suivi des pics d'une trame à l'autre : seul un pic persistant et stable en
//!    niveau devient un parasite (une note de piano décroît, un sifflement non) ;
//! 4. pour chaque parasite, amplitude et phase ajustées par moindres carrés sur
//!    les cases voisines, puis synthèse de la sinusoïde fenêtrée dans la trame.
//!
//! La sinusoïde synthétisée est ensuite soustraite : c'est une annulation par
//! opposition de phase de l'onde elle-même, sans creuser la bande de fréquences.

use crate::stft::sqrt_hann;
use realfft::num_complex::{Complex32, Complex64};
use std::f64::consts::PI;

const MAX_TRACKS: usize = 16;
const MAX_CANDIDATES: usize = 32;
/// Cases utilisées de part et d'autre du pic pour l'ajustement.
const FIT_HALF: isize = 4;
/// Demi-largeur (en cases) de la moyenne glissante qui sert de voisinage.
const ENVELOPE_HALF: usize = 12;
/// Étendue et résolution de la table de la transformée de la fenêtre.
const TABLE_SPAN: f64 = 24.0;
const TABLE_RES: f64 = 256.0;
/// Trames manquées tolérées avant d'abandonner un suivi.
const MAX_MISSED: u32 = 3;
/// Écart maximal (en cases) pour rattacher un pic à un suivi.
const MATCH_BINS: f32 = 0.75;
/// Variation de poids par trame (≈ 5 trames pour un fondu complet).
const WEIGHT_STEP: f32 = 0.2;
/// Marge au-dessus de l'enveloppe des lobes secondaires d'un pic plus fort.
const SIDELOBE_MARGIN_DB: f32 = 10.0;
/// Part maximale d'énergie locale non expliquée par la sinusoïde pour faire
/// confiance à l'ajustement ; au-delà (attaque, partiel voisin), on prolonge la
/// sinusoïde de la trame précédente.
const MAX_UNEXPLAINED: f64 = 0.05;
/// Prolongation maximale, en trames (≈ 50 ms à 48 kHz).
const MAX_PREDICTED: u32 = 5;
/// Vitesse de relâchement des enveloppes de niveau, en dB/s.
const ENVELOPE_RELEASE_DB_S: f32 = 3.0;
/// Écart maximal entre enveloppes haute et basse pour un parasite, en dB.
const MAX_RANGE_DB: f32 = 4.0;

#[derive(Clone, Copy, Debug)]
pub struct ResonanceSettings {
    pub enabled: bool,
    /// Proéminence minimale d'un pic au-dessus de son voisinage, en dB.
    pub sensitivity_db: f32,
    /// Durée minimale de présence avant soustraction, en secondes.
    pub persistence_s: f32,
    /// Variation de niveau maximale d'un parasite, en dB/s.
    pub max_slope_db_s: f32,
    pub min_hz: f32,
    pub max_hz: f32,
}

impl Default for ResonanceSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            sensitivity_db: 15.0,
            persistence_s: 0.3,
            max_slope_db_s: 4.0,
            min_hz: 40.0,
            max_hz: 18_000.0,
        }
    }
}

/// Résonance en cours de soustraction, pour l'affichage.
#[derive(Clone, Copy, Debug, Default)]
pub struct ResonanceInfo {
    pub hz: f32,
    /// Amplitude de la sinusoïde, en dBFS.
    pub level_db: f32,
    /// Part soustraite, de 0 à 1 (fondus d'entrée et de sortie).
    pub weight: f32,
}

#[derive(Clone, Copy, Default)]
struct Track {
    alive: bool,
    f: f32,
    age: u32,
    missed: u32,
    amp_db: f32,
    slope: f32,
    weight: f32,
    /// Amplitude complexe utilisée à la trame précédente.
    c: Complex64,
    has_c: bool,
    /// Mesures de niveau fiables depuis la naissance du suivi.
    clean_count: u32,
    /// Enveloppes haute et basse du niveau (dB), relâchées lentement : leur
    /// écart mesure la stabilité sur les dernières centaines de ms.
    hi_db: f32,
    lo_db: f32,
    /// Trames consécutives où la sinusoïde a été prolongée.
    predicted: u32,
}

#[derive(Clone, Copy, Default)]
struct Candidate {
    f: f32,
    amp_db: f32,
    prominence: f32,
    /// L'ajustement explique presque toute l'énergie locale (pas de perturbation).
    clean: bool,
    used: bool,
}

pub struct ResonanceEngine {
    n: usize,
    hop: usize,
    sample_rate: f32,
    window: Vec<f32>,
    /// Wd(δ) = Σ w[n] e^(−j2πδn/N) pour δ ∈ [0, TABLE_SPAN].
    table: Vec<Complex64>,
    prev_phase: Vec<f32>,
    mag_db: Vec<f32>,
    cumsum: Vec<f64>,
    candidates: Vec<Candidate>,
    tracks: [Track; MAX_TRACKS],
    settings: ResonanceSettings,
    has_prev: bool,
}

impl ResonanceEngine {
    pub fn new(n: usize, hop: usize, sample_rate: f32) -> Self {
        let window = sqrt_hann(n);
        let entries = (TABLE_SPAN * TABLE_RES) as usize + 2;
        let table = (0..entries)
            .map(|i| {
                let delta = i as f64 / TABLE_RES;
                let rot = Complex64::from_polar(1.0, -2.0 * PI * delta / n as f64);
                let mut z = Complex64::new(1.0, 0.0);
                let mut acc = Complex64::new(0.0, 0.0);
                for &w in &window {
                    acc += z * w as f64;
                    z *= rot;
                }
                acc
            })
            .collect();
        let bins = n / 2 + 1;
        Self {
            n,
            hop,
            sample_rate,
            window,
            table,
            prev_phase: vec![0.0; bins],
            mag_db: vec![0.0; bins],
            cumsum: vec![0.0; bins + 1],
            candidates: Vec::with_capacity(MAX_CANDIDATES),
            tracks: [Track::default(); MAX_TRACKS],
            settings: ResonanceSettings::default(),
            has_prev: false,
        }
    }

    pub fn set_settings(&mut self, s: ResonanceSettings) {
        self.settings = s;
    }

    pub fn reset(&mut self) {
        self.tracks = [Track::default(); MAX_TRACKS];
        self.has_prev = false;
    }

    /// Remplit `out` avec les résonances soustraites ; renvoie leur nombre.
    pub fn resonances(&self, out: &mut [ResonanceInfo]) -> usize {
        let bin_hz = self.sample_rate / self.n as f32;
        let mut count = 0;
        for t in self.tracks.iter().filter(|t| t.alive && t.weight > 0.0) {
            if count == out.len() {
                break;
            }
            out[count] = ResonanceInfo { hz: t.f * bin_hz, level_db: t.amp_db, weight: t.weight };
            count += 1;
        }
        count
    }

    /// Nombre de parasites en cours de soustraction (pour l'affichage et les tests).
    pub fn active_count(&self) -> usize {
        self.tracks.iter().filter(|t| t.alive && t.weight > 0.0).count()
    }

    fn wd(&self, delta: f64) -> Complex64 {
        let a = delta.abs();
        if a >= TABLE_SPAN {
            return Complex64::new(0.0, 0.0);
        }
        let pos = a * TABLE_RES;
        let i = pos as usize;
        let frac = pos - i as f64;
        let v = self.table[i] * (1.0 - frac) + self.table[i + 1] * frac;
        if delta < 0.0 {
            v.conj()
        } else {
            v
        }
    }

    /// Ajuste Re(c · e^(j2πfn/N)) sur les cases voisines de f.
    /// Renvoie (c, énergie résiduelle, énergie totale des cases).
    fn fit(&self, spectrum: &[Complex32], f: f32) -> (Complex64, f64, f64) {
        let bins = spectrum.len() as isize;
        let k0 = f.round() as isize;
        let lo = (k0 - FIT_HALF).max(1);
        let hi = (k0 + FIT_HALF).min(bins - 2);
        let (mut a11, mut a12, mut a22, mut b1, mut b2, mut xx) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for k in lo..=hi {
            let x = Complex64::new(spectrum[k as usize].re as f64, spectrum[k as usize].im as f64);
            let a = self.wd(k as f64 - f as f64) * 0.5;
            let b = self.wd(k as f64 + f as f64) * 0.5;
            let e = a + b;
            let d = a - b;
            let g = Complex64::new(-d.im, d.re); // j·(a − b)
            a11 += e.norm_sqr();
            a12 += (e.conj() * g).re;
            a22 += g.norm_sqr();
            b1 += (e.conj() * x).re;
            b2 += (g.conj() * x).re;
            xx += x.norm_sqr();
        }
        let det = a11 * a22 - a12 * a12;
        if det.abs() < 1e-20 {
            return (Complex64::new(0.0, 0.0), xx, xx);
        }
        let u = (b1 * a22 - b2 * a12) / det;
        let v = (a11 * b2 - a12 * b1) / det;
        (Complex64::new(u, v), (xx - (u * b1 + v * b2)).max(0.0), xx)
    }

    /// Affine f en minimisant l'erreur d'ajustement (deux passes paraboliques).
    fn refine(&self, spectrum: &[Complex32], mut f: f32) -> f32 {
        for d in [0.02f32, 0.004] {
            let em = self.fit(spectrum, f - d).1;
            let e0 = self.fit(spectrum, f).1;
            let ep = self.fit(spectrum, f + d).1;
            let denom = em - 2.0 * e0 + ep;
            if denom > 0.0 {
                let off = (0.5 * (em - ep) / denom) as f32 * d;
                f += off.clamp(-d, d);
            }
        }
        f
    }

    fn detect(&mut self, spectrum: &[Complex32]) {
        let bins = spectrum.len();
        for (m, x) in self.mag_db.iter_mut().zip(spectrum) {
            *m = 10.0 * (x.norm_sqr() + 1e-20).log10();
        }
        self.cumsum[0] = 0.0;
        for k in 0..bins {
            self.cumsum[k + 1] = self.cumsum[k] + self.mag_db[k] as f64;
        }

        let bin_hz = self.sample_rate / self.n as f32;
        let k_min = ((self.settings.min_hz / bin_hz).ceil() as usize).max(3);
        let k_max = ((self.settings.max_hz / bin_hz).floor() as usize).min(bins.saturating_sub(4));
        // Plancher absolu : sinusoïde à −80 dBFS.
        let floor_db = 20.0 * (1e-4 * self.n as f32 / std::f32::consts::PI).log10();
        let expected_step = 2.0 * std::f32::consts::PI * self.hop as f32 / self.n as f32;

        self.candidates.clear();
        for k in k_min..=k_max.max(k_min) {
            if k + 2 >= bins {
                break;
            }
            let m = self.mag_db[k];
            if m < floor_db
                || m <= self.mag_db[k - 1]
                || m < self.mag_db[k + 1]
                || m < self.mag_db[k - 2]
                || m < self.mag_db[k + 2]
            {
                continue;
            }
            let lo = k.saturating_sub(ENVELOPE_HALF);
            let hi = (k + ENVELOPE_HALF + 1).min(bins);
            let envelope = ((self.cumsum[hi] - self.cumsum[lo]) / (hi - lo) as f64) as f32;
            let prominence = m - envelope;
            if prominence < self.settings.sensitivity_db {
                continue;
            }
            // Fréquence fine par la différence de phase entre deux trames.
            let mut f = k as f32;
            if self.has_prev {
                let phase = spectrum[k].im.atan2(spectrum[k].re);
                let dev = wrap(phase - self.prev_phase[k] - expected_step * k as f32);
                f += dev / expected_step;
            }
            let cand = Candidate { f, amp_db: m, prominence, clean: false, used: false };
            if self.candidates.len() < MAX_CANDIDATES {
                self.candidates.push(cand);
            } else if let Some(min) = self
                .candidates
                .iter_mut()
                .min_by(|a, b| a.prominence.total_cmp(&b.prominence))
            {
                if min.prominence < prominence {
                    *min = cand;
                }
            }
        }

        // Les lobes secondaires d'un pic fort ne sont pas des résonances : on
        // écarte tout pic qui ne dépasse pas nettement l'enveloppe des lobes
        // secondaires (≈ 1 / (4δ² − 1) pour la fenêtre racine de Hann) d'un pic
        // plus fort.
        self.candidates.sort_unstable_by(|a, b| b.amp_db.total_cmp(&a.amp_db));
        let mut kept = 0;
        for i in 0..self.candidates.len() {
            let c = self.candidates[i];
            let masked = self.candidates[..kept].iter().any(|q| {
                let d = (c.f - q.f).abs();
                d < 1.5 || c.amp_db < q.amp_db - 20.0 * (4.0 * d * d - 1.0).log10() + SIDELOBE_MARGIN_DB
            });
            if !masked {
                self.candidates[kept] = c;
                kept += 1;
            }
        }
        self.candidates.truncate(kept);

        // Niveau mesuré par l'amplitude ajustée, indépendante de la position
        // du pic entre deux cases.
        for i in 0..self.candidates.len() {
            let (c, resid, xx) = self.fit(spectrum, self.candidates[i].f);
            self.candidates[i].amp_db = 20.0 * (c.norm() as f32 + 1e-12).log10();
            self.candidates[i].clean = resid < MAX_UNEXPLAINED * xx;
        }

        for (p, x) in self.prev_phase.iter_mut().zip(spectrum) {
            *p = x.im.atan2(x.re);
        }
        self.has_prev = true;
    }

    fn update_tracks(&mut self) {
        let dt = self.hop as f32 / self.sample_rate;
        let persist_frames = (self.settings.persistence_s / dt).ceil() as u32;

        for t in self.tracks.iter_mut().filter(|t| t.alive) {
            let best = self
                .candidates
                .iter_mut()
                .filter(|c| !c.used && (c.f - t.f).abs() < MATCH_BINS)
                .min_by(|a, b| (a.f - t.f).abs().total_cmp(&(b.f - t.f).abs()));
            t.age = t.age.saturating_add(1);
            match best {
                Some(c) => {
                    c.used = true;
                    if c.clean {
                        let inst = (c.amp_db - t.amp_db) / dt;
                        t.slope += 0.1 * (inst - t.slope);
                        t.amp_db = c.amp_db;
                        t.clean_count += 1;
                        let rel = ENVELOPE_RELEASE_DB_S * dt;
                        t.hi_db = c.amp_db.max(t.hi_db - rel);
                        t.lo_db = c.amp_db.min(t.lo_db + rel);
                    }
                    t.f = c.f;
                    t.missed = 0;
                }
                None => t.missed += 1,
            }
            let eligible = self.settings.enabled
                && t.missed <= MAX_MISSED
                && t.age >= persist_frames
                && t.clean_count >= persist_frames / 2
                && t.slope.abs() <= self.settings.max_slope_db_s
                && t.hi_db - t.lo_db <= MAX_RANGE_DB;
            let target = if eligible { 1.0 } else { 0.0 };
            t.weight += (target - t.weight).clamp(-WEIGHT_STEP, WEIGHT_STEP);
            if t.missed > MAX_MISSED && t.weight <= 0.0 {
                t.alive = false;
            }
        }

        // Deux suivis sur la même sinusoïde se soustrairaient deux fois : on
        // garde le plus ancien.
        for i in 0..MAX_TRACKS {
            for j in 0..MAX_TRACKS {
                let (a, b) = (self.tracks[i], self.tracks[j]);
                if i != j && a.alive && b.alive && (a.f - b.f).abs() < 1.5 && (b.age, j) < (a.age, i) {
                    self.tracks[j].alive = false;
                }
            }
        }

        for c in self.candidates.iter().filter(|c| !c.used) {
            if self.tracks.iter().any(|t| t.alive && (t.f - c.f).abs() < 1.5) {
                continue;
            }
            if let Some(slot) = self.tracks.iter_mut().find(|t| !t.alive) {
                *slot = Track {
                    alive: true,
                    f: c.f,
                    amp_db: c.amp_db,
                    hi_db: c.amp_db,
                    lo_db: c.amp_db,
                    ..Track::default()
                };
            }
        }
    }

    /// Ajoute dans `frame` (domaine temporel, fenêtré) les sinusoïdes parasites.
    pub fn estimate(&mut self, spectrum: &[Complex32], frame: &mut [f32]) {
        self.detect(spectrum);
        self.update_tracks();

        for i in 0..MAX_TRACKS {
            let t = self.tracks[i];
            if !t.alive || t.weight <= 0.0 {
                continue;
            }
            let advance = Complex64::from_polar(1.0, 2.0 * PI * t.f as f64 * self.hop as f64 / self.n as f64);
            let refined = self.refine(spectrum, t.f);
            let (fitted, resid, xx) = self.fit(spectrum, refined);
            let disturbed = resid >= MAX_UNEXPLAINED * xx;
            let (f, c) = if t.has_c && disturbed && t.predicted < MAX_PREDICTED {
                // Perturbation brève (attaque) : on prolonge la sinusoïde précédente.
                self.tracks[i].predicted += 1;
                (t.f, t.c * advance)
            } else {
                self.tracks[i].predicted = 0;
                (refined, fitted)
            };
            self.tracks[i].f = f;
            self.tracks[i].c = c;
            self.tracks[i].has_c = true;
            let rot = Complex64::from_polar(1.0, 2.0 * PI * f as f64 / self.n as f64);
            let mut z = c * t.weight as f64;
            for (out, w) in frame.iter_mut().zip(&self.window) {
                *out += w * z.re as f32;
                z *= rot;
            }
        }
    }
}

fn wrap(x: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    x - TAU * ((x + PI) / TAU).floor()
}
