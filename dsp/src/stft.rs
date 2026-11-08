//! STFT à recouvrement-addition qui reconstruit la composante parasite p.
//!
//! Pour chaque trame, un [`Estimator`] décrit le parasite de deux façons
//! cumulables :
//! - un spectre parasite P(k) (par exemple (1 − G) × X pour le bruit) ;
//! - une trame temporelle déjà fenêtrée par la fenêtre d'analyse (sinusoïdes
//!   synthétisées par le moteur de résonances).
//!
//! Fenêtres d'analyse et de synthèse en racine de Hann, pas de N/4 : un parasite
//! égal à la trame d'entrée redonne l'entrée retardée de exactement `latency()`.

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

/// Décrit la composante parasite d'une trame.
pub trait Estimator {
    /// `spectrum` : X(k) de la trame fenêtrée. `parasite` (spectre) et
    /// `frame` (temporel, fenêtré par l'analyse) arrivent à zéro.
    fn estimate(&mut self, spectrum: &[Complex32], parasite: &mut [Complex32], frame: &mut [f32]);
}

/// Rien n'est retiré : p = 0.
pub struct KeepAll;
impl Estimator for KeepAll {
    fn estimate(&mut self, _: &[Complex32], _: &mut [Complex32], _: &mut [f32]) {}
}

/// Tout est retiré : p = entrée retardée (test d'annulation).
pub struct RemoveAll;
impl Estimator for RemoveAll {
    fn estimate(&mut self, spectrum: &[Complex32], parasite: &mut [Complex32], _: &mut [f32]) {
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
    out_fifo: Vec<f32>,
    out_acc: Vec<f32>,
    rover: usize,
    frame: Vec<f32>,
    frame_td: Vec<f32>,
    spectrum: Vec<Complex32>,
    parasite: Vec<Complex32>,
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
            out_fifo: vec![0.0; hop],
            out_acc: vec![0.0; n],
            rover: n - hop,
            frame: vec![0.0; n],
            frame_td: vec![0.0; n],
            spectrum: vec![Complex32::new(0.0, 0.0); bins],
            parasite: vec![Complex32::new(0.0, 0.0); bins],
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

    pub fn reset(&mut self) {
        self.in_fifo.fill(0.0);
        self.out_fifo.fill(0.0);
        self.out_acc.fill(0.0);
        self.rover = self.n - self.hop;
    }

    /// Ajoute un échantillon d'entrée et renvoie l'échantillon de p retardé.
    #[inline]
    pub fn process(&mut self, x: f32, est: &mut impl Estimator) -> f32 {
        self.in_fifo[self.rover] = x;
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

        self.parasite.fill(Complex32::new(0.0, 0.0));
        self.frame_td.fill(0.0);
        est.estimate(&self.spectrum, &mut self.parasite, &mut self.frame_td);
        let last = self.parasite.len() - 1;
        self.parasite[0].im = 0.0;
        self.parasite[last].im = 0.0;

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
    }
}
