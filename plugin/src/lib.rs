//! SoundScalpel : nettoyage audio par soustraction en inversion de phase.
//!
//! Usage prévu : Audio Effect Rack à deux chaînes, « Original » vide et
//! « SoundScalpel » en mode « Parasite inversé ». La somme du rack donne le son
//! nettoyé. Moteurs : résonances et sifflements (synthèse et soustraction).
//! Le moteur de bruit viendra ensuite.

use nih_plug::prelude::*;
use soundscalpel_dsp::resonance::ResonanceSettings;
use soundscalpel_dsp::{fft_size_for, ChannelProcessor, Output, Settings};
use std::num::NonZeroU32;
use std::sync::Arc;

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
enum OutputMode {
    #[name = "Parasite inversé"]
    InvertedParasite,
    #[name = "Signal nettoyé"]
    Cleaned,
}

#[derive(Params)]
struct ScalpelParams {
    #[id = "intensity"]
    intensity: FloatParam,
    #[id = "output"]
    output: EnumParam<OutputMode>,
    #[id = "listen"]
    listen: BoolParam,
    #[id = "cancel_test"]
    cancel_test: BoolParam,
    #[id = "bypass"]
    bypass: BoolParam,

    #[id = "res_on"]
    res_enabled: BoolParam,
    #[id = "res_sens"]
    res_sensitivity: FloatParam,
    #[id = "res_persist"]
    res_persistence: FloatParam,
    #[id = "res_slope"]
    res_max_slope: FloatParam,
    #[id = "res_lo"]
    res_min_hz: FloatParam,
    #[id = "res_hi"]
    res_max_hz: FloatParam,
}

fn hz_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Skewed { min, max, factor: FloatRange::skew_factor(-2.0) })
        .with_value_to_string(formatters::v2s_f32_hz_then_khz(0))
        .with_string_to_value(formatters::s2v_f32_hz_then_khz())
}

impl Default for ScalpelParams {
    fn default() -> Self {
        Self {
            intensity: FloatParam::new("Intensité", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_smoother(SmoothingStyle::Linear(20.0))
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            output: EnumParam::new("Mode de sortie", OutputMode::InvertedParasite),
            listen: BoolParam::new("Écouter ce qui est retiré", false).non_automatable(),
            cancel_test: BoolParam::new("Test d'annulation", false).non_automatable(),
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
        }
    }
}

struct SoundScalpel {
    params: Arc<ScalpelParams>,
    channels: Vec<ChannelProcessor>,
}

impl Default for SoundScalpel {
    fn default() -> Self {
        Self { params: Arc::new(ScalpelParams::default()), channels: Vec::new() }
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
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn initialize(
        &mut self,
        layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let channels = layout.main_output_channels.map(NonZeroU32::get).unwrap_or(0) as usize;
        let n = fft_size_for(buffer_config.sample_rate);
        let sr = buffer_config.sample_rate;
        self.channels = (0..channels).map(|_| ChannelProcessor::new(n, sr)).collect();
        let latency = self.channels.first().map(ChannelProcessor::latency).unwrap_or(0);
        context.set_latency_samples(latency as u32);
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
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let p = &self.params;
        let rack = p.output.value() == OutputMode::InvertedParasite;
        let out = if p.bypass.value() {
            if rack { Output::BypassSilent } else { Output::BypassDry }
        } else if p.cancel_test.value() {
            Output::CancelTest
        } else if p.listen.value() {
            Output::Listen
        } else if rack {
            Output::InvertedParasite
        } else {
            Output::Cleaned
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
        };
        for ch in &mut self.channels {
            ch.set_settings(&settings);
        }

        for mut frame in buffer.iter_samples() {
            let a = p.intensity.smoothed.next();
            for (ch, sample) in self.channels.iter_mut().zip(frame.iter_mut()) {
                *sample = ch.process(*sample, a, out);
            }
        }
        ProcessStatus::Normal
    }
}

impl Vst3Plugin for SoundScalpel {
    const VST3_CLASS_ID: [u8; 16] = *b"SoundScalpelSrtz";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Restoration];
}

nih_export_vst3!(SoundScalpel);
