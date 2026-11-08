use super::*;
use resonance::ResonanceInfo;
use stft::KeepAll;

const SR: f32 = 48_000.0;

/// Bruit pseudo-aléatoire reproductible.
struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn noise_and_tone(len: usize) -> Vec<f32> {
    let mut rng = Rng(0x1234_5678);
    (0..len)
        .map(|i| 0.25 * (0.5 * rng.next() + 0.5 * (i as f32 * 0.031).sin()))
        .collect()
}

/// Piano de synthèse : notes de 2 s toutes les 0,5 s, 10 partiels inharmoniques
/// qui décroissent, attaque de 5 ms. Crête ≈ −12 dBFS.
fn synthetic_piano(len: usize) -> Vec<f32> {
    let notes = [220.0f64, 277.18, 329.63, 440.0, 523.25];
    let b = 1e-4;
    let mut out = vec![0.0f64; len];
    let step = (0.5 * SR as f64) as usize;
    let dur = (2.0 * SR as f64) as usize;
    let mut start = 0;
    let mut idx = 0;
    while start < len {
        let f0 = notes[idx % notes.len()];
        for h in 1..=10 {
            let hf = h as f64;
            let f = hf * f0 * (1.0 + b * hf * hf).sqrt();
            let tau = 0.8 / hf.sqrt();
            let amp = 1.0 / hf;
            let phase = 0.37 * hf + idx as f64;
            for t in 0..dur.min(len - start) {
                let ts = t as f64 / SR as f64;
                let env = (ts / 0.005).min(1.0) * (-ts / tau).exp();
                out[start + t] += amp * env * (2.0 * std::f64::consts::PI * f * ts + phase).sin();
            }
        }
        start += step;
        idx += 1;
    }
    let peak = out.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    out.iter().map(|v| (v / peak * 0.25) as f32).collect()
}

fn sine(len: usize, hz: f32, amp: f32) -> Vec<f32> {
    (0..len)
        .map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / SR + 0.3).sin())
        .collect()
}

fn rms(v: impl Iterator<Item = f32>) -> f64 {
    let (s, n) = v.fold((0.0f64, 0usize), |(s, n), x| (s + (x as f64).powi(2), n + 1));
    (s / n.max(1) as f64).sqrt()
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-15).log10()
}

/// Passe le signal dans un canal et renvoie p = ce qui est retiré (a = 1).
fn removed(x: &[f32]) -> (Vec<f32>, usize) {
    let mut ch = ChannelProcessor::new(2048, SR);
    ch.set_settings(&Settings::default());
    let lat = ch.latency();
    (x.iter().map(|&v| ch.process(v, 1.0, Output::Listen)).collect(), lat)
}

// ---------- Phase 1 : chaîne neutre ----------

#[test]
fn reconstruction_neutre_sous_moins_100_db() {
    for n in [2048, 4096] {
        let x = noise_and_tone(48_000);
        let mut stft = Stft::new(n);
        let lat = stft.latency();
        let p: Vec<f32> = x.iter().map(|&v| stft.process(v, &mut RemoveAll)).collect();
        let err = db(rms((2 * n..x.len()).map(|t| p[t] - x[t - lat])));
        assert!(err < -100.0, "N = {n} : erreur RMS {err:.1} dBFS");
    }
}

#[test]
fn sans_parasite_p_est_nul() {
    let x = noise_and_tone(20_000);
    let mut stft = Stft::new(2048);
    assert!(x.iter().all(|&v| stft.process(v, &mut KeepAll) == 0.0));
}

#[test]
fn test_d_annulation_donne_un_silence_dans_le_rack() {
    let x = noise_and_tone(48_000);
    let mut ch = ChannelProcessor::new(2048, SR);
    let lat = ch.latency();
    let sum: Vec<f32> = x
        .iter()
        .enumerate()
        .map(|(t, &v)| {
            let original = if t >= lat { x[t - lat] } else { 0.0 };
            original + ch.process(v, 0.3, Output::CancelTest)
        })
        .collect();
    assert!(db(rms(sum[4096..].iter().copied())) < -100.0);
}

#[test]
fn coherence_rack_et_signal_nettoye() {
    let len = 60_000;
    let x: Vec<f32> = synthetic_piano(len)
        .iter()
        .zip(sine(len, 6000.0, 0.0316))
        .map(|(a, b)| a + b)
        .collect();
    let mut a = ChannelProcessor::new(2048, SR);
    let mut b = ChannelProcessor::new(2048, SR);
    let mut c = ChannelProcessor::new(2048, SR);
    let mut d = ChannelProcessor::new(2048, SR);
    let lat = a.latency();
    for (t, &v) in x.iter().enumerate() {
        let original = if t >= lat { x[t - lat] } else { 0.0 };
        let rack = original + a.process(v, 0.7, Output::InvertedParasite);
        let cleaned = b.process(v, 0.7, Output::Cleaned);
        let listened = c.process(v, 0.7, Output::Listen);
        assert!((rack - cleaned).abs() < 1e-5, "t = {t}");
        assert!((cleaned + listened - original).abs() < 1e-5, "t = {t}");
        let rack_listen = original + d.process(v, 0.7, Output::ListenRack);
        assert!((rack_listen - listened).abs() < 1e-5, "t = {t}");
    }
}

#[test]
fn valeurs_non_finies_neutralisees() {
    let mut ch = ChannelProcessor::new(2048, SR);
    for i in 0..10_000 {
        let v = if i % 97 == 0 { f32::NAN } else { 0.1 };
        assert!(ch.process(v, 1.0, Output::Cleaned).is_finite());
    }
}

// ---------- Moteur 1 : résonances ----------

/// Amplitude de la composante à `hz` (projection sur sinus et cosinus).
fn tone_amplitude(v: &[f32], hz: f32, start: usize) -> f64 {
    let (mut s, mut c) = (0.0f64, 0.0f64);
    for (i, &x) in v.iter().enumerate() {
        let ph = 2.0 * std::f64::consts::PI * hz as f64 * (start + i) as f64 / SR as f64;
        s += x as f64 * ph.sin();
        c += x as f64 * ph.cos();
    }
    2.0 * (s * s + c * c).sqrt() / v.len() as f64
}

/// Recette « Sifflement isolé » : piano + sinus 6 kHz à −30 dBFS.
#[test]
fn sifflement_isole_reduit_d_au_moins_30_db() {
    let len = 5 * SR as usize;
    let piano = synthetic_piano(len);
    let whistle = sine(len, 6000.0, 0.0316);
    let x: Vec<f32> = piano.iter().zip(&whistle).map(|(a, b)| a + b).collect();
    let (p, lat) = removed(&x);

    let from = 2 * SR as usize; // après détection et confirmation
    // Sortie nettoyée − piano retardé = sifflement restant + ce qui a été pris au piano.
    let left: Vec<f32> = (from..len).map(|t| x[t - lat] - p[t] - piano[t - lat]).collect();
    let reduction = db(0.0316) - db(tone_amplitude(&left, 6000.0, from - lat));
    // Ce qui a été retiré en plus du sifflement.
    let collateral = db(rms((from..len).map(|t| p[t] - whistle[t - lat])))
        - db(rms((from..len).map(|t| piano[t - lat])));
    println!("réduction du sifflement : {reduction:.1} dB ; retrait hors sifflement : {collateral:.1} dB / piano");
    assert!(reduction >= 30.0, "réduction {reduction:.1} dB");
    assert!(collateral < -30.0, "piano touché : {collateral:.1} dB");
}

/// Un piano seul ne doit presque rien perdre : ses partiels décroissent.
#[test]
fn piano_seul_presque_intact() {
    let len = 5 * SR as usize;
    let piano = synthetic_piano(len);
    let (p, lat) = removed(&piano);
    let from = SR as usize;
    let ratio = db(rms(p[from..].iter().copied())) - db(rms((from..len).map(|t| piano[t - lat])));
    println!("retiré sur piano seul : {ratio:.1} dB");
    assert!(ratio < -30.0, "retiré {ratio:.1} dB");
}

/// Sifflement qui glisse lentement en fréquence (± 20 Hz sur 2 s) : il doit être suivi.
#[test]
fn sifflement_qui_derive_est_suivi() {
    let len = 4 * SR as usize;
    let mut phase = 0.0f64;
    let whistle: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f64 / SR as f64;
            let f = 3000.0 + 20.0 * (std::f64::consts::PI * t).sin();
            phase += 2.0 * std::f64::consts::PI * f / SR as f64;
            (0.05 * phase.sin()) as f32
        })
        .collect();
    let (p, lat) = removed(&whistle);
    let from = 2 * SR as usize;
    let level = rms((from..len).map(|t| whistle[t - lat]));
    let left = rms((from..len).map(|t| whistle[t - lat] - p[t]));
    let reduction = db(level) - db(left);
    println!("réduction du sifflement glissant : {reduction:.1} dB");
    assert!(reduction >= 25.0, "réduction {reduction:.1} dB");
}

/// Désactivé : rien n'est retiré.
#[test]
fn moteur_desactive_ne_retire_rien() {
    let len = 3 * SR as usize;
    let x = sine(len, 6000.0, 0.0316);
    let mut ch = ChannelProcessor::new(2048, SR);
    let mut s = Settings::default();
    s.resonance.enabled = false;
    ch.set_settings(&s);
    let p: Vec<f32> = x.iter().map(|&v| ch.process(v, 1.0, Output::Listen)).collect();
    assert!(p.iter().all(|&v| v == 0.0));
}

/// Sinus seul à différentes fréquences : annulation profonde.
#[test]
fn sinus_pur_annule_d_au_moins_40_db() {
    let len = 3 * SR as usize;
    for hz in [150.0f32, 3000.0, 6000.0] {
        let x = sine(len, hz, 0.0316);
        let (p, lat) = removed(&x);
        let from = 2 * SR as usize;
        let r = db(rms((from..len).map(|t| x[t - lat]))) - db(rms((from..len).map(|t| x[t - lat] - p[t])));
        println!("sinus pur {hz} Hz : {r:.1} dB");
        assert!(r >= 40.0, "{hz} Hz : {r:.1} dB");
    }
}

#[test]
fn test_d_annulation_direct_donne_un_silence() {
    let x = noise_and_tone(48_000);
    let mut ch = ChannelProcessor::new(2048, SR);
    let y: Vec<f32> = x.iter().map(|&v| ch.process(v, 1.0, Output::CancelTestDirect)).collect();
    assert!(db(rms(y[4096..].iter().copied())) < -100.0);
}

/// L'affichage voit le sifflement dans « retiré » et plus dans la sortie.
#[test]
fn spectres_d_affichage_coherents() {
    let len = 3 * SR as usize;
    let x = sine(len, 6000.0, 0.0316);
    let mut ch = ChannelProcessor::new(2048, SR);
    for &v in &x {
        ch.process(v, 1.0, Output::Cleaned);
    }
    let k = (6000.0 / (SR / 2048.0)).round() as usize;
    let (inp, rem, out) = ch.spectra();
    assert!(rem[k] > 0.9 * inp[k], "retiré {} / entrée {}", rem[k], inp[k]);
    assert!(out[k] < 1e-4 * inp[k], "sortie {} / entrée {}", out[k], inp[k]);
    let mut info = [ResonanceInfo::default(); 4];
    let n = ch.resonances(&mut info);
    assert_eq!(n, 1);
    assert!((info[0].hz - 6000.0).abs() < 1.0, "{} Hz", info[0].hz);
    assert!((info[0].level_db - db(0.0316) as f32).abs() < 0.5, "{} dB", info[0].level_db);
}
