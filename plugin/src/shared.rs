//! État partagé entre le fil audio et l'interface.
//!
//! Le fil audio ne bloque jamais : il tente de prendre le verrou de l'affichage
//! et renonce si l'interface le tient (une image perdue, pas de décrochage).

use soundscalpel_dsp::noise::NoiseStatus;
use soundscalpel_dsp::resonance::ResonanceInfo;
use soundscalpel_dsp::stem::StemStatus;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

/// Nombre maximal de résonances affichées.
pub const MAX_SHOWN: usize = 32;

pub struct Shared {
    /// Écoute de ce qui est retiré. Volontairement hors paramètres : non
    /// enregistrée avec le projet, qui se rouvre donc en écoute normale.
    pub listen: AtomicBool,
    /// Test d'annulation, même règle que l'écoute.
    pub cancel_test: AtomicBool,
    /// Demande de remise à zéro du profil de bruit. Action instantanée, sans
    /// état à mémoriser : le fil audio l'exécute puis le remet lui-même à
    /// false (bouton « à bascule qui se relâche tout seul »).
    pub noise_reset: AtomicBool,
    /// Latence déclarée, en échantillons.
    pub latency: AtomicU32,
    pub display: Mutex<Display>,
}

#[derive(Default)]
pub struct Display {
    pub sample_rate: f32,
    pub fft_size: usize,
    /// Puissances par case, moyennées sur les canaux.
    pub input: Vec<f32>,
    pub removed: Vec<f32>,
    pub output: Vec<f32>,
    pub resonances: Vec<ResonanceInfo>,
    /// État de la sidechain (restauration de stem, mode A).
    pub stem: StemStatus,
    /// État du profil de bruit.
    pub noise: NoiseStatus,
    /// Incrémenté à chaque nouvelle image.
    pub version: u64,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            listen: AtomicBool::new(false),
            cancel_test: AtomicBool::new(false),
            noise_reset: AtomicBool::new(false),
            latency: AtomicU32::new(0),
            display: Mutex::new(Display::default()),
        }
    }
}

impl Shared {
    pub fn is_listening(&self) -> bool {
        self.listen.load(Ordering::Relaxed)
    }
    pub fn is_cancel_test(&self) -> bool {
        self.cancel_test.load(Ordering::Relaxed)
    }
}

impl Display {
    /// Dimensionne les tampons (hors fil audio, à l'initialisation).
    pub fn prepare(&mut self, sample_rate: f32, fft_size: usize) {
        let bins = fft_size / 2 + 1;
        self.sample_rate = sample_rate;
        self.fft_size = fft_size;
        for v in [&mut self.input, &mut self.removed, &mut self.output] {
            v.clear();
            v.resize(bins, 0.0);
        }
        self.resonances.clear();
        self.resonances.reserve(MAX_SHOWN);
    }
}
