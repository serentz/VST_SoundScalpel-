//! Moteur audio de SoundScalpel, indépendant de l'hôte et testable hors plugin.
//!
//! Principe (cahier des charges, section 3) : le plugin estime la composante
//! parasite p et, dans un Audio Effect Rack à deux chaînes, sort −a × p ; la
//! somme avec la chaîne « Original » donne x − a × p.

pub mod resonance;
pub mod stft;

use realfft::num_complex::Complex32;
use resonance::{ResonanceEngine, ResonanceInfo, ResonanceSettings};
use stft::{Estimator, RemoveAll, Stft};

/// Ce que le canal envoie en sortie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// −a × p : chaîne SoundScalpel du rack (mode par défaut).
    InvertedParasite,
    /// xD − a × p : insertion directe sans rack.
    Cleaned,
    /// a × p : écoute de ce qui est retiré, en insertion directe.
    Listen,
    /// a × p − xD : dans le rack, la somme avec la chaîne « Original » donne a × p.
    ListenRack,
    /// −xD reconstruit par la STFT : la somme du rack doit être un silence.
    CancelTest,
    /// xD − p avec tout retiré : silence en insertion directe.
    CancelTestDirect,
    /// Contournement en mode rack : silence, la chaîne « Original » passe seule.
    BypassSilent,
    /// Contournement en insertion directe : entrée retardée.
    BypassDry,
}

/// Taille de FFT selon la fréquence d'échantillonnage : la fenêtre garde la même
/// durée (≈ 43 ms) et la même résolution en Hz.
pub fn fft_size_for(sample_rate: f32) -> usize {
    if sample_rate > 60_000.0 {
        4096
    } else {
        2048
    }
}

/// Ligne à retard fixe qui aligne l'entrée sur p.
pub struct Delay {
    buf: Vec<f32>,
    pos: usize,
}

impl Delay {
    pub fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], pos: 0 }
    }
    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.buf[self.pos];
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

/// Réglages de traitement d'un canal.
#[derive(Clone, Copy, Debug, Default)]
pub struct Settings {
    pub resonance: ResonanceSettings,
}

/// Ensemble des moteurs. Le moteur de bruit viendra s'ajouter ici.
pub struct Engines {
    resonance: ResonanceEngine,
}

impl Estimator for Engines {
    fn estimate(&mut self, spectrum: &[Complex32], _parasite: &mut [Complex32], frame: &mut [f32]) {
        self.resonance.estimate(spectrum, frame);
    }
}

/// Traitement complet d'un canal.
pub struct ChannelProcessor {
    stft: Stft,
    delay: Delay,
    engines: Engines,
}

impl ChannelProcessor {
    pub fn new(fft_size: usize, sample_rate: f32) -> Self {
        let stft = Stft::new(fft_size);
        let delay = Delay::new(stft.latency());
        let resonance = ResonanceEngine::new(fft_size, stft.hop(), sample_rate);
        Self { stft, delay, engines: Engines { resonance } }
    }

    pub fn latency(&self) -> usize {
        self.stft.latency()
    }

    pub fn set_settings(&mut self, s: &Settings) {
        self.engines.resonance.set_settings(s.resonance);
    }

    /// Puissances par case (entrée, retiré, sortie) de la dernière trame.
    pub fn spectra(&self) -> (&[f32], &[f32], &[f32]) {
        self.stft.spectra()
    }

    /// Nombre de trames traitées.
    pub fn frames(&self) -> u64 {
        self.stft.frames()
    }

    pub fn fft_size(&self) -> usize {
        self.stft.fft_size()
    }

    /// Résonances soustraites ; renvoie leur nombre.
    pub fn resonances(&self, out: &mut [ResonanceInfo]) -> usize {
        self.engines.resonance.resonances(out)
    }

    /// Nombre de résonances en cours de soustraction.
    pub fn active_resonances(&self) -> usize {
        self.engines.resonance.active_count()
    }

    pub fn reset(&mut self) {
        self.stft.reset();
        self.delay.reset();
        self.engines.resonance.reset();
    }

    /// `a` : intensité entre 0 et 1.
    #[inline]
    pub fn process(&mut self, x: f32, a: f32, out: Output) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        let xd = self.delay.process(x);
        let p = if matches!(out, Output::CancelTest | Output::CancelTestDirect) {
            self.stft.process(x, &mut RemoveAll)
        } else {
            self.stft.process(x, &mut self.engines)
        };
        let y = match out {
            Output::InvertedParasite => -a * p,
            Output::Cleaned => xd - a * p,
            Output::Listen => a * p,
            Output::ListenRack => a * p - xd,
            Output::CancelTest => -p,
            Output::CancelTestDirect => xd - p,
            Output::BypassSilent => 0.0,
            Output::BypassDry => xd,
        };
        // Retour contrôlé vers un signal sûr si le calcul a dérivé.
        if y.is_finite() {
            y
        } else {
            match out {
                Output::Cleaned | Output::BypassDry => xd,
                _ => 0.0,
            }
        }
    }
}

#[cfg(test)]
mod tests;
