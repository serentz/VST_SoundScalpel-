//! Moteur audio de SoundScalpel, indépendant de l'hôte et testable hors plugin.
//!
//! Principe (cahier des charges, section 3) : le plugin estime la composante
//! parasite p et, dans un Audio Effect Rack à deux chaînes, sort −a × p ; la
//! somme avec la chaîne « Original » donne x − a × p.

pub mod noise;
pub mod resonance;
pub mod stem;
pub mod stft;

use realfft::num_complex::Complex32;
use noise::{NoiseEngine, NoiseSettings, NoiseStatus};
use resonance::{ResonanceEngine, ResonanceInfo, ResonanceSettings, MAX_TRACKS};
use stem::{StemEngine, StemSettings, StemStatus};
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
    pub noise: NoiseSettings,
    pub stem: StemSettings,
}

/// Ensemble des moteurs.
pub struct Engines {
    resonance: ResonanceEngine,
    noise: NoiseEngine,
    stem: StemEngine,
    /// Fréquences des résonances actives, recalculées à chaque trame : le
    /// moteur de bruit les exclut de son masque pour ne pas les retirer une
    /// seconde fois (pas d'allocation : tampon fixe, réutilisé).
    resonance_info: [ResonanceInfo; MAX_TRACKS],
    resonance_hz: [f32; MAX_TRACKS],
}

impl Estimator for Engines {
    fn estimate(
        &mut self,
        spectrum: &[Complex32],
        reference: Option<&[Complex32]>,
        parasite: &mut [Complex32],
        frame: &mut [f32],
    ) {
        self.resonance.estimate(spectrum, frame);
        let n = self.resonance.resonances(&mut self.resonance_info);
        for i in 0..n {
            self.resonance_hz[i] = self.resonance_info[i].hz;
        }
        self.noise.estimate(spectrum, &self.resonance_hz[..n], parasite);

        // Plafond global, pour le seul moteur de bruit (le masque de
        // résonances ne touche pas `parasite`, et les cases qu'il traite sont
        // déjà exclues par le moteur de bruit ci-dessus) : il ne peut pas
        // retirer plus que le signal observé à une case donnée. Ce plafond ne
        // s'applique pas à la restauration de stem, qui ajoute volontairement
        // de l'énergie là où le signal d'origine est faible ou nul — le
        // limiter à |X| viderait sa correction exactement là où elle sert.
        for (p, x) in parasite.iter_mut().zip(spectrum) {
            let (pm, xm) = (p.norm(), x.norm());
            if pm > xm && pm > 1e-20 {
                *p *= xm / pm;
            }
        }

        self.stem.estimate(spectrum, reference, parasite);
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
        let noise = NoiseEngine::new(fft_size, stft.hop(), sample_rate);
        let stem = StemEngine::new(fft_size, sample_rate);
        let engines = Engines {
            resonance,
            noise,
            stem,
            resonance_info: [ResonanceInfo::default(); MAX_TRACKS],
            resonance_hz: [0.0; MAX_TRACKS],
        };
        Self { stft, delay, engines }
    }

    pub fn latency(&self) -> usize {
        self.stft.latency()
    }

    pub fn set_settings(&mut self, s: &Settings) {
        self.engines.resonance.set_settings(s.resonance);
        self.engines.noise.set_settings(s.noise);
        self.engines.stem.set_settings(s.stem);
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

    /// État de la sidechain pour la restauration de stem.
    pub fn stem_status(&self) -> StemStatus {
        self.engines.stem.status()
    }

    /// État du profil de bruit.
    pub fn noise_status(&self) -> NoiseStatus {
        self.engines.noise.status()
    }

    /// Efface le profil de bruit appris et l'estimation automatique.
    pub fn reset_noise_profile(&mut self) {
        self.engines.noise.reset_profile();
    }

    /// Nombre de résonances en cours de soustraction.
    pub fn active_resonances(&self) -> usize {
        self.engines.resonance.active_count()
    }

    pub fn reset(&mut self) {
        self.stft.reset();
        self.delay.reset();
        self.engines.resonance.reset();
        self.engines.noise.reset();
        self.engines.stem.reset();
    }

    /// `a` : intensité entre 0 et 1.
    #[inline]
    pub fn process(&mut self, x: f32, a: f32, out: Output) -> f32 {
        self.process_with_reference(x, None, a, out)
    }

    /// Comme [`ChannelProcessor::process`], avec un échantillon du mix d'origine
    /// (sidechain) aligné sur l'entrée.
    #[inline]
    pub fn process_with_reference(&mut self, x: f32, reference: Option<f32>, a: f32, out: Output) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        let reference = reference.map(|r| if r.is_finite() { r } else { 0.0 });
        let xd = self.delay.process(x);
        let p = if matches!(out, Output::CancelTest | Output::CancelTestDirect) {
            self.stft.process(x, &mut RemoveAll)
        } else {
            self.stft.process_with_reference(x, reference, &mut self.engines)
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
