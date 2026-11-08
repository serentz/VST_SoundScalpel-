//! SoundScalpel : nettoyage audio par soustraction en inversion de phase.
//!
//! Usage prévu : Audio Effect Rack à deux chaînes, « Original » vide et
//! « SoundScalpel » en mode « Parasite inversé ». La somme du rack donne le son
//! nettoyé. Moteurs : résonances et sifflements (synthèse et soustraction),
//! restauration de stem (mode A avec le mix d'origine sur la sidechain, mode B
//! sans référence). Le moteur de bruit viendra ensuite.

mod editor;
mod shared;

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use shared::{Shared, MAX_SHOWN};
use soundscalpel_dsp::resonance::{ResonanceInfo, ResonanceSettings};
use soundscalpel_dsp::stem::{StemMode, StemSettings};
use soundscalpel_dsp::{fft_size_for, ChannelProcessor, Output, Settings};
use std::num::NonZeroU32;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum OutputMode {
    #[name = "Parasite inversé"]
    InvertedParasite,
    #[name = "Signal nettoyé"]
    Cleaned,
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum StemChoice {
    #[name = "Désactivée"]
    Off,
    #[name = "A : avec le mix d'origine"]
    WithMix,
    #[name = "B : sans référence"]
    Blind,
}

#[derive(Params)]
pub struct ScalpelParams {
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,

    #[id = "intensity"]
    pub intensity: FloatParam,
    #[id = "output"]
    pub output: EnumParam<OutputMode>,
    #[id = "bypass"]
    pub bypass: BoolParam,

    #[id = "res_on"]
    pub res_enabled: BoolParam,
    #[id = "res_sens"]
    pub res_sensitivity: FloatParam,
    #[id = "res_persist"]
    pub res_persistence: FloatParam,
    #[id = "res_slope"]
    pub res_max_slope: FloatParam,
    #[id = "res_lo"]
    pub res_min_hz: FloatParam,
    #[id = "res_hi"]
    pub res_max_hz: FloatParam,

    #[id = "stem_mode"]
    pub stem_mode: EnumParam<StemChoice>,
    #[id = "stem_start"]
    pub stem_start: FloatParam,
    #[id = "stem_recovery"]
    pub stem_recovery: FloatParam,
    #[id = "stem_slope"]
    pub stem_slope: FloatParam,
    #[id = "stem_boost"]
    pub stem_boost: FloatParam,
    #[id = "stem_synth"]
    pub stem_synth: FloatParam,
}

fn percent_param(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_unit(" %")
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

fn hz_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Skewed { min, max, factor: FloatRange::skew_factor(-2.0) })
        .with_value_to_string(formatters::v2s_f32_hz_then_khz(0))
        .with_string_to_value(formatters::s2v_f32_hz_then_khz())
}

impl Default for ScalpelParams {
    fn default() -> Self {
        Self {
            editor_state: editor::default_state(),
            intensity: FloatParam::new("Intensité", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_smoother(SmoothingStyle::Linear(20.0))
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            output: EnumParam::new("Mode de sortie", OutputMode::InvertedParasite),
            bypass: BoolParam::new("Contournement", false).make_bypass(),

            res_enabled: BoolParam::new("Résonances", true),
            res_sensitivity: FloatParam::new(
                "Sensibilité résonances",
                15.0,
                FloatRange::Linear { min: 6.0, max: 40.0 },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            res_persistence: FloatParam::new(
                "Persistance",
                300.0,
                FloatRange::Skewed { min: 50.0, max: 2000.0, factor: FloatRange::skew_factor(-1.0) },
            )
            .with_unit(" ms")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            res_max_slope: FloatParam::new(
                "Variation tolérée",
                4.0,
                FloatRange::Skewed { min: 0.5, max: 20.0, factor: FloatRange::skew_factor(-1.0) },
            )
            .with_unit(" dB/s")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            res_min_hz: hz_param("Fréquence basse", 40.0, 20.0, 2000.0),
            res_max_hz: hz_param("Fréquence haute", 18_000.0, 1000.0, 20_000.0),

            stem_mode: EnumParam::new("Restauration de stem", StemChoice::Off),
            stem_start: hz_param("Début de restauration", 2500.0, 500.0, 10_000.0),
            stem_recovery: percent_param("Récupération", 1.0),
            stem_slope: FloatParam::new("Pente cible", -4.5, FloatRange::Linear { min: -12.0, max: 0.0 })
                .with_unit(" dB/oct")
                .with_value_to_string(formatters::v2s_f32_rounded(1)),
            stem_boost: FloatParam::new("Remontée maximale", 12.0, FloatRange::Linear { min: 0.0, max: 24.0 })
                .with_unit(" dB")
                .with_value_to_string(formatters::v2s_f32_rounded(1)),
            stem_synth: percent_param("Aigus recréés", 0.0),
        }
    }
}

const PORT_NAMES: PortNames = PortNames {
    layout: None,
    main_input: None,
    main_output: None,
    aux_inputs: &["Mix d'origine"],
    aux_outputs: &[],
};

struct SoundScalpel {
    params: Arc<ScalpelParams>,
    shared: Arc<Shared>,
    channels: Vec<ChannelProcessor>,
    last_frames: u64,
    infos: [ResonanceInfo; MAX_SHOWN],
}

impl Default for SoundScalpel {
    fn default() -> Self {
        Self {
            params: Arc::new(ScalpelParams::default()),
            shared: Arc::new(Shared::default()),
            channels: Vec::new(),
            last_frames: 0,
            infos: [ResonanceInfo::default(); MAX_SHOWN],
        }
    }
}

impl SoundScalpel {
    /// Copie la dernière trame analysée vers l'interface, sans jamais attendre.
    fn publish_display(&mut self) {
        let Some(first) = self.channels.first() else { return };
        let frames = first.frames();
        let first_status = first.stem_status();
        if frames == self.last_frames {
            return;
        }
        let Ok(mut d) = self.shared.display.try_lock() else { return };
        self.last_frames = frames;
        let scale = 1.0 / self.channels.len() as f32;
        d.input.fill(0.0);
        d.removed.fill(0.0);
        d.output.fill(0.0);
        d.resonances.clear();
        let bin_hz = d.sample_rate / d.fft_size.max(1) as f32;
        for ch in &self.channels {
            let (i, r, o) = ch.spectra();
            let n = d.input.len().min(i.len());
            for k in 0..n {
                d.input[k] += i[k] * scale;
                d.removed[k] += r[k] * scale;
                d.output[k] += o[k] * scale;
            }
            let count = ch.resonances(&mut self.infos);
            for info in &self.infos[..count] {
                // En stéréo, la même résonance apparaît sur les deux canaux.
                if let Some(same) = d.resonances.iter_mut().find(|r| (r.hz - info.hz).abs() < bin_hz) {
                    same.weight = same.weight.max(info.weight);
                    same.level_db = same.level_db.max(info.level_db);
                } else if d.resonances.len() < MAX_SHOWN {
                    d.resonances.push(*info);
                }
            }
        }
        d.stem = first_status;
        d.version = d.version.wrapping_add(1);
    }
}

impl Plugin for SoundScalpel {
    const NAME: &'static str = "SoundScalpel";
    const VENDOR: &'static str = "Serentz";
    const URL: &'static str = "https://github.com/serentz/VST_SoundScalpel-";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            aux_input_ports: &[new_nonzero_u32(2)],
            names: PORT_NAMES,
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            aux_input_ports: &[new_nonzero_u32(1)],
            names: PORT_NAMES,
            ..AudioIOLayout::const_default()
        },
    ];

    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.shared.clone())
    }

    fn initialize(
        &mut self,
        layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let channels = layout.main_output_channels.map(NonZeroU32::get).unwrap_or(0) as usize;
        let sr = buffer_config.sample_rate;
        let n = fft_size_for(sr);
        self.channels = (0..channels).map(|_| ChannelProcessor::new(n, sr)).collect();
        self.last_frames = 0;
        let latency = self.channels.first().map(ChannelProcessor::latency).unwrap_or(0) as u32;
        context.set_latency_samples(latency);
        self.shared.latency.store(latency, Ordering::Relaxed);
        if let Ok(mut d) = self.shared.display.lock() {
            d.prepare(sr, n);
        }
        true
    }

    fn reset(&mut self) {
        for ch in &mut self.channels {
            ch.reset();
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let p = &self.params;
        let rack = p.output.value() == OutputMode::InvertedParasite;
        let out = match (p.bypass.value(), self.shared.is_cancel_test(), self.shared.is_listening(), rack) {
            (true, _, _, true) => Output::BypassSilent,
            (true, _, _, false) => Output::BypassDry,
            (_, true, _, true) => Output::CancelTest,
            (_, true, _, false) => Output::CancelTestDirect,
            (_, _, true, true) => Output::ListenRack,
            (_, _, true, false) => Output::Listen,
            (_, _, _, true) => Output::InvertedParasite,
            (_, _, _, false) => Output::Cleaned,
        };

        let settings = Settings {
            resonance: ResonanceSettings {
                enabled: p.res_enabled.value(),
                sensitivity_db: p.res_sensitivity.value(),
                persistence_s: p.res_persistence.value() / 1000.0,
                max_slope_db_s: p.res_max_slope.value(),
                min_hz: p.res_min_hz.value(),
                max_hz: p.res_max_hz.value(),
            },
            stem: StemSettings {
                mode: match p.stem_mode.value() {
                    StemChoice::Off => StemMode::Off,
                    StemChoice::WithMix => StemMode::WithMix,
                    StemChoice::Blind => StemMode::Blind,
                },
                start_hz: p.stem_start.value(),
                recovery: p.stem_recovery.value(),
                target_slope_db_oct: p.stem_slope.value(),
                max_boost_db: p.stem_boost.value(),
                synth: p.stem_synth.value(),
            },
        };
        // Le mix d'origine n'est analysé qu'en mode A.
        let reference = if settings.stem.mode == StemMode::WithMix {
            aux.inputs.first().map(|b| b.as_slice_immutable()).filter(|c| !c.is_empty())
        } else {
            None
        };
        for ch in &mut self.channels {
            ch.set_settings(&settings);
        }

        for (t, mut frame) in buffer.iter_samples().enumerate() {
            let a = p.intensity.smoothed.next();
            for (c, (ch, sample)) in self.channels.iter_mut().zip(frame.iter_mut()).enumerate() {
                let r = reference.and_then(|aux| aux[c.min(aux.len() - 1)].get(t).copied());
                *sample = ch.process_with_reference(*sample, r, a, out);
            }
        }

        self.publish_display();
        ProcessStatus::Normal
    }
}

impl Vst3Plugin for SoundScalpel {
    const VST3_CLASS_ID: [u8; 16] = *b"SoundScalpelSrtz";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Restoration];
}

nih_export_vst3!(SoundScalpel);
