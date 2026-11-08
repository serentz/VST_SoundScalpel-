//! STFT à recouvrement-addition qui reconstruit la composante parasite p.
//!
//! Pour chaque trame, un [`Estimator`] décrit le parasite de deux façons
//! cumulables :
//! - un spectre parasite P(k) (par exemple (1 − G) × X pour le bruit) ;
//! - une trame temporelle déjà fenêtrée par la fenêtre d'analyse (sinusoïdes
//!   synthétisées par le moteur de résonances).
//!
//! Une entrée de référence facultative (le mix d'origine, reçu par la
//! sidechain) est analysée trame par trame en même temps que l'entrée.
//!
//! Fenêtres d'analyse et de synthèse en racine de Hann, pas de N/4 : un parasite
//! égal à la trame d'entrée redonne l'entrée retardée de exactement `latency()`.

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

/// Décrit la composante parasite d'une trame.
pub trait Estimator {
    /// `spectrum` : X(k) de la trame fenêtrée ; `reference` : spectre de la
    /// référence alignée, si elle est fournie. `parasite` (spectre) et `frame`
    /// (temporel, fenêtré par l'analyse) arrivent à zéro.
    fn estimate(
        &mut self,
        spectrum: &[Complex32],
        reference: Option<&[Complex32]>,
        parasite: &mut [Complex32],
        frame: &mut [f32],
    );
}

/// Rien n'est retiré : p = 0.
pub struct KeepAll;
impl Estimator for KeepAll {
    fn estimate(&mut self, _: &[Complex32], _: Option<&[Complex32]>, _: &mut [Complex32], _: &mut [f32]) {}
}

/// Tout est retiré : p = entrée retardée (test d'annulation).
pub struct RemoveAll;
impl Estimator for RemoveAll {
    fn estimate(&mut self, spectrum: &[Complex32], _: Option<&[Complex32]>, parasite: &mut [Complex32], _: &mut [f32]) {
        parasite.copy_from_slice(spectrum);
    }
}

/// Racine de Hann périodique : sin(π n / N).
pub fn sqrt_hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (std::f64::consts::PI * i as f64 / n as f64).sin() as f32)
        .collect()
}

pub struct Stft {
    n: usize,
    hop: usize,
    window: Vec<f32>,
    inv_overlap: f32,
    in_fifo: Vec<f32>,
    ref_fifo: Vec<f32>,
    ref_spectrum: Vec<Complex32>,
    /// Une référence a été fournie depuis la dernière trame.
    ref_seen: bool,
    out_fifo: Vec<f32>,
    out_acc: Vec<f32>,
    rover: usize,
    frame: Vec<f32>,
    frame_td: Vec<f32>,
    spectrum: Vec<Complex32>,
    parasite: Vec<Complex32>,
    removed: Vec<Complex32>,
    td_spectrum: Vec<Complex32>,
    /// Puissances par case de la dernière trame : entrée, retiré, sortie nettoyée.
    in_pow: Vec<f32>,
    removed_pow: Vec<f32>,
    out_pow: Vec<f32>,
    frames: u64,
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    scratch_fwd: Vec<Complex32>,
    scratch_inv: Vec<Complex32>,
}

impl Stft {
    /// `n` : taille de FFT (puissance de 2, ≥ 16). Toutes les allocations ont lieu ici.
    pub fn new(n: usize) -> Self {
        assert!(n.is_power_of_two() && n >= 16);
        let hop = n / 4;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let ifft = planner.plan_fft_inverse(n);
        // Analyse × synthèse = Hann, dont les copies décalées de N/4 somment à 2.
        let overlap_sum = 0.5 * n as f32 / hop as f32;
        let bins = n / 2 + 1;
        Self {
            n,
            hop,
            window: sqrt_hann(n),
            inv_overlap: 1.0 / overlap_sum,
            in_fifo: vec![0.0; n],
            ref_fifo: vec![0.0; n],
            ref_spectrum: vec![Complex32::new(0.0, 0.0); bins],
            ref_seen: false,
            out_fifo: vec![0.0; hop],
            out_acc: vec![0.0; n],
            rover: n - hop,
            frame: vec![0.0; n],
            frame_td: vec![0.0; n],
            spectrum: vec![Complex32::new(0.0, 0.0); bins],
            parasite: vec![Complex32::new(0.0, 0.0); bins],
            removed: vec![Complex32::new(0.0, 0.0); bins],
            td_spectrum: vec![Complex32::new(0.0, 0.0); bins],
            in_pow: vec![0.0; bins],
            removed_pow: vec![0.0; bins],
            out_pow: vec![0.0; bins],
            frames: 0,
            scratch_fwd: fft.make_scratch_vec(),
            scratch_inv: ifft.make_scratch_vec(),
            fft,
            ifft,
        }
    }

    /// Latence de p par rapport à l'entrée, en échantillons.
    pub fn latency(&self) -> usize {
        self.n
    }

    pub fn fft_size(&self) -> usize {
        self.n
    }

    pub fn hop(&self) -> usize {
        self.hop
    }

    /// Puissances par case (entrée, retiré, sortie) de la dernière trame.
    pub fn spectra(&self) -> (&[f32], &[f32], &[f32]) {
        (&self.in_pow, &self.removed_pow, &self.out_pow)
    }

    /// Nombre de trames traitées depuis la création.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn reset(&mut self) {
        self.in_fifo.fill(0.0);
        self.ref_fifo.fill(0.0);
        self.out_fifo.fill(0.0);
        self.out_acc.fill(0.0);
        self.rover = self.n - self.hop;
    }

    /// Ajoute un échantillon d'entrée et renvoie l'échantillon de p retardé.
    #[inline]
    pub fn process(&mut self, x: f32, est: &mut impl Estimator) -> f32 {
        self.process_with_reference(x, None, est)
    }

    /// Comme [`Stft::process`], avec un échantillon de la référence alignée.
    #[inline]
    pub fn process_with_reference(&mut self, x: f32, reference: Option<f32>, est: &mut impl Estimator) -> f32 {
        self.in_fifo[self.rover] = x;
        if let Some(r) = reference {
            self.ref_fifo[self.rover] = r;
            self.ref_seen = true;
        } else {
            self.ref_fifo[self.rover] = 0.0;
        }
        let p = self.out_fifo[self.rover - (self.n - self.hop)];
        self.rover += 1;
        if self.rover == self.n {
            self.process_frame(est);
            self.rover = self.n - self.hop;
        }
        p
    }

    fn process_frame(&mut self, est: &mut impl Estimator) {
        for ((f, x), w) in self.frame.iter_mut().zip(&self.in_fifo).zip(&self.window) {
            *f = x * w;
        }
        // Les longueurs sont fixées à la construction : ces appels ne peuvent pas échouer.
        let _ = self
            .fft
            .process_with_scratch(&mut self.frame, &mut self.spectrum, &mut self.scratch_fwd);

        let has_ref = self.ref_seen;
        if has_ref {
            for ((f, x), w) in self.frame.iter_mut().zip(&self.ref_fifo).zip(&self.window) {
                *f = x * w;
            }
            let _ = self
                .fft
                .process_with_scratch(&mut self.frame, &mut self.ref_spectrum, &mut self.scratch_fwd);
        }
        self.ref_seen = false;

        self.parasite.fill(Complex32::new(0.0, 0.0));
        self.frame_td.fill(0.0);
        let reference = if has_ref { Some(&self.ref_spectrum[..]) } else { None };
        est.estimate(&self.spectrum, reference, &mut self.parasite, &mut self.frame_td);
        let last = self.parasite.len() - 1;
        self.parasite[0].im = 0.0;
        self.parasite[last].im = 0.0;

        // Spectre complet de ce qui est retiré, pour l'affichage.
        self.removed.copy_from_slice(&self.parasite);
        self.frame.copy_from_slice(&self.frame_td);
        let _ = self
            .fft
            .process_with_scratch(&mut self.frame, &mut self.td_spectrum, &mut self.scratch_fwd);
        for k in 0..self.removed.len() {
            let r = self.removed[k] + self.td_spectrum[k];
            let x = self.spectrum[k];
            self.in_pow[k] = x.norm_sqr();
            self.removed_pow[k] = r.norm_sqr();
            self.out_pow[k] = (x - r).norm_sqr();
        }
        self.frames += 1;

        let _ = self
            .ifft
            .process_with_scratch(&mut self.parasite, &mut self.frame, &mut self.scratch_inv);
        let inv_n = 1.0 / self.n as f32; // l'IFFT de realfft n'est pas normalisée
        for (((acc, f), td), w) in self
            .out_acc
            .iter_mut()
            .zip(&self.frame)
            .zip(&self.frame_td)
            .zip(&self.window)
        {
            *acc += (f * inv_n + td) * w * self.inv_overlap;
        }

        let hop = self.hop;
        self.out_fifo.copy_from_slice(&self.out_acc[..hop]);
        self.out_acc.copy_within(hop.., 0);
        let n = self.n;
        self.out_acc[n - hop..].fill(0.0);
        self.in_fifo.copy_within(hop.., 0);
        self.ref_fifo.copy_within(hop.., 0);
    }
}
