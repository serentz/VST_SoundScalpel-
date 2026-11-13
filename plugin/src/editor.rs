//! Interface graphique (egui) : analyseur central, intensité et écoute du retiré
//! toujours visibles, réglages avancés, liste des résonances soustraites.

use crate::shared::Shared;
use crate::{OutputMode, ScalpelParams, StemChoice};
use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use nih_plug_egui::{create_egui_editor, widgets, EguiState};
use soundscalpel_dsp::resonance::ResonanceInfo;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub const WIDTH: u32 = 1000;
pub const HEIGHT: u32 = 720;

const BG: Color32 = Color32::from_rgb(0x14, 0x12, 0x1B);
const PANEL: Color32 = Color32::from_rgb(0x1E, 0x1B, 0x28);
const GRID: Color32 = Color32::from_rgb(0x2E, 0x2A, 0x3B);
const LABEL: Color32 = Color32::from_rgb(0x8C, 0x86, 0xA3);
const INPUT: Color32 = Color32::from_rgb(0x3F, 0x39, 0x55);
const OUTPUT: Color32 = Color32::from_rgb(0xD8, 0xD2, 0xF0);
const ACCENT: Color32 = Color32::from_rgb(0xFF, 0x6A, 0x2C);
const LISTEN: Color32 = Color32::from_rgb(0xD9, 0x2B, 0x3A);
const TEST: Color32 = Color32::from_rgb(0x2B, 0x8C, 0xD9);
const OK: Color32 = Color32::from_rgb(0x4C, 0xC2, 0x7A);
const WARN: Color32 = Color32::from_rgb(0xF2, 0xB1, 0x3A);

const MIN_HZ: f32 = 20.0;
const MAX_HZ: f32 = 20_000.0;
const TOP_DB: f32 = 0.0;
const BOTTOM_DB: f32 = -110.0;
/// Retombée de l'affichage par image (≈ 60 images/s).
const RELEASE_DB: f32 = 1.2;

pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(WIDTH, HEIGHT)
}

/// Section ouverte dans le panneau de droite. Une seule à la fois : chacune a
/// ainsi toute la hauteur disponible, sans avoir à faire défiler l'autre pour
/// l'atteindre.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Resonance,
    Stem,
}

impl Default for Section {
    fn default() -> Self {
        Section::Resonance
    }
}

/// Copie lissée de l'affichage, propre à l'interface.
#[derive(Default)]
struct View {
    version: u64,
    sample_rate: f32,
    fft_size: usize,
    input: Vec<f32>,
    removed: Vec<f32>,
    output: Vec<f32>,
    resonances: Vec<ResonanceInfo>,
    /// Images affichées sans nouvelle donnée audio.
    stale: u32,
    section: Section,
}

impl View {
    /// Vrai quand aucun son n'arrive (lecture arrêtée ou piste muette).
    fn is_silent(&self) -> bool {
        self.input.iter().all(|&v| v <= BOTTOM_DB + 5.0)
    }

    fn refresh(&mut self, shared: &Shared) {
        let Ok(d) = shared.display.lock() else { return };
        if d.version == self.version || d.input.is_empty() {
            // Plus de données (Live n'appelle plus le plugin) : l'affichage retombe.
            self.stale = self.stale.saturating_add(1);
            for v in [&mut self.input, &mut self.removed, &mut self.output] {
                for x in v.iter_mut() {
                    *x = (*x - RELEASE_DB).max(BOTTOM_DB);
                }
            }
            if self.stale > 30 {
                self.resonances.clear();
            }
            return;
        }
        self.stale = 0;
        self.version = d.version;
        if self.fft_size != d.fft_size || self.input.len() != d.input.len() {
            self.fft_size = d.fft_size;
            self.sample_rate = d.sample_rate;
            for v in [&mut self.input, &mut self.removed, &mut self.output] {
                v.clear();
                v.resize(d.input.len(), BOTTOM_DB);
            }
        }
        // Puissance d'une trame fenêtrée → amplitude sinusoïdale en dBFS.
        let norm = 20.0 * (std::f32::consts::PI / d.fft_size.max(1) as f32).log10();
        let smooth = |dst: &mut [f32], src: &[f32]| {
            for (v, p) in dst.iter_mut().zip(src) {
                let db = 10.0 * (p + 1e-20).log10() + norm;
                *v = db.max(*v - RELEASE_DB);
            }
        };
        smooth(&mut self.input, &d.input);
        smooth(&mut self.removed, &d.removed);
        smooth(&mut self.output, &d.output);
        self.resonances.clear();
        self.resonances.extend_from_slice(&d.resonances);
        self.resonances.sort_by(|a, b| b.level_db.total_cmp(&a.level_db));
    }
}

pub fn create(params: Arc<ScalpelParams>, shared: Arc<Shared>) -> Option<Box<dyn Editor>> {
    create_egui_editor(
        params.editor_state.clone(),
        View::default(),
        |ctx, _| {
            let mut style = (*ctx.style()).clone();
            style.visuals = egui::Visuals::dark();
            style.visuals.panel_fill = PANEL;
            style.visuals.selection.bg_fill = ACCENT;
            ctx.set_style(style);
        },
        move |ctx, setter, view| {
            ctx.request_repaint();
            view.refresh(&shared);

            egui::SidePanel::right("controls")
                .exact_width(290.0)
                .resizable(false)
                .show(ctx, |ui| controls(ui, &params, setter, &shared, &mut view.section));

            egui::TopBottomPanel::bottom("resonances")
                .exact_height(54.0)
                .show(ctx, |ui| resonance_list(ui, view));

            egui::CentralPanel::default().show(ctx, |ui| analyzer(ui, view, &params, &shared));
        },
    )
}

fn set_param<P: Param>(setter: &ParamSetter, param: &P, value: P::Plain) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

/// Nombre à la française : virgule décimale.
fn fr(v: f32, decimals: usize) -> String {
    format!("{v:.decimals$}").replace('.', ",")
}

fn hz_label(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{} kHz", fr(hz / 1000.0, if hz >= 10_000.0 { 1 } else { 2 }))
    } else {
        format!("{} Hz", fr(hz, 0))
    }
}

fn big_toggle(ui: &mut egui::Ui, on: bool, text_off: &str, text_on: &str, color: Color32, height: f32) -> bool {
    let text = if on { text_on } else { text_off };
    let button = egui::Button::new(RichText::new(text).strong().size(14.0).color(if on {
        Color32::WHITE
    } else {
        OUTPUT
    }))
    .fill(if on { color } else { GRID })
    .min_size(Vec2::new((ui.available_width() - 6.0).max(40.0), height));
    ui.add(button).clicked()
}

/// Libellé et valeur lisible sur une ligne, curseur pleine largeur dessous.
fn slider(ui: &mut egui::Ui, label: &str, param: &FloatParam, setter: &ParamSetter) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(LABEL));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format!("{param}")).color(OUTPUT).strong());
        });
    });
    let width = (ui.available_width() - 6.0).max(40.0);
    ui.add(widgets::ParamSlider::for_param(param, setter).without_value().with_width(width));
    ui.add_space(2.0);
}

fn controls(ui: &mut egui::Ui, params: &ScalpelParams, setter: &ParamSetter, shared: &Shared, section: &mut Section) {
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 5.0);
    ui.add_space(6.0);
    ui.label(RichText::new("SoundScalpel").size(22.0).strong().color(ACCENT));
    ui.label(RichText::new("Soustraction par inversion de phase").color(LABEL));
    ui.add_space(8.0);

    slider(ui, "Intensité", &params.intensity, setter);
    ui.add_space(4.0);

    let listening = shared.is_listening();
    if big_toggle(ui, listening, "ÉCOUTER CE QUI EST RETIRÉ", "ÉCOUTE DU RETIRÉ : ACTIVE", LISTEN, 42.0) {
        shared.listen.store(!listening, Ordering::Relaxed);
    }
    ui.add_space(8.0);

    ui.label(RichText::new("Sortie").color(LABEL));
    ui.horizontal(|ui| {
        let current = params.output.value();
        for (mode, name, hint) in [
            (OutputMode::InvertedParasite, "Parasite inversé", "Dans un rack à deux chaînes : l'autre chaîne reste vide"),
            (OutputMode::Cleaned, "Signal nettoyé", "Insertion directe, sans rack"),
        ] {
            if ui.selectable_label(current == mode, name).on_hover_text(hint).clicked() {
                set_param(setter, &params.output, mode);
            }
        }
    });
    ui.add_space(4.0);

    let testing = shared.is_cancel_test();
    if big_toggle(ui, testing, "Test d'annulation", "Test en cours : la piste doit être muette", TEST, 26.0) {
        shared.cancel_test.store(!testing, Ordering::Relaxed);
    }

    let latency = shared.latency.load(Ordering::Relaxed);
    let sr = shared.display.lock().map(|d| d.sample_rate).unwrap_or(0.0);
    let ms = if sr > 0.0 { latency as f32 * 1000.0 / sr } else { 0.0 };
    ui.label(RichText::new(format!("Latence déclarée : {latency} éch. ({} ms)", fr(ms, 1))).color(LABEL));

    ui.add_space(6.0);
    ui.separator();

    // Deux onglets plutôt qu'une pile : chaque section a toute la hauteur
    // restante, au lieu d'avoir à faire défiler l'une pour atteindre l'autre.
    ui.horizontal(|ui| {
        if ui.selectable_label(*section == Section::Resonance, "Résonances").clicked() {
            *section = Section::Resonance;
        }
        if ui.selectable_label(*section == Section::Stem, "Restauration de stem").clicked() {
            *section = Section::Stem;
        }
    });
    ui.add_space(4.0);

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match section {
        Section::Resonance => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Résonances et sifflements").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let on = params.res_enabled.value();
                    if ui.selectable_label(on, if on { "Actif" } else { "Inactif" }).clicked() {
                        set_param(setter, &params.res_enabled, !on);
                    }
                });
            });
            ui.add_space(4.0);
            slider(ui, "Sensibilité (écart au voisinage)", &params.res_sensitivity, setter);
            slider(ui, "Persistance avant retrait", &params.res_persistence, setter);
            slider(ui, "Variation de niveau tolérée", &params.res_max_slope, setter);
            slider(ui, "Fréquence basse", &params.res_min_hz, setter);
            slider(ui, "Fréquence haute", &params.res_max_hz, setter);
        }
        Section::Stem => stem_section(ui, params, setter, shared),
    });
}

fn stem_section(ui: &mut egui::Ui, params: &ScalpelParams, setter: &ParamSetter, shared: &Shared) {
    ui.label(RichText::new("Restauration de stem (aigus perdus)").strong());
    let current = params.stem_mode.value();
    ui.horizontal_wrapped(|ui| {
        for (mode, name, hint) in [
            (StemChoice::Off, "Désactivée", "Aucune restauration"),
            (
                StemChoice::WithMix,
                "A : avec le mix",
                "Le mix d'origine arrive sur la sidechain du plugin : les aigus perdus sont repris dans le mix, avec leur vraie phase",
            ),
            (
                StemChoice::Blind,
                "B : sans référence",
                "Sans le mix d'origine : remontée dynamique des aigus qui s'effondrent, aigus recréés en option",
            ),
        ] {
            if ui.selectable_label(current == mode, name).on_hover_text(hint).clicked() {
                set_param(setter, &params.stem_mode, mode);
            }
        }
    });
    ui.add_space(2.0);
    match current {
        StemChoice::Off => {}
        StemChoice::WithMix => {
            let status = shared.display.lock().map(|d| d.stem).unwrap_or_default();
            let (text, color) = if !status.has_reference {
                (
                    "Aucun signal sur la sidechain : dans Live, choisis la piste du mix d'origine dans « Sidechain » du plugin."
                        .to_string(),
                    WARN,
                )
            } else if !status.lag_reliable {
                ("Mix reçu, mesure de l'alignement en cours…".to_string(), LABEL)
            } else if status.lag == 0 {
                ("Mix reçu, aligné sur le stem.".to_string(), OK)
            } else {
                let side = if status.lag > 0 { "en retard" } else { "en avance" };
                (format!("Stem {side} de {} éch. sur le mix : aligne les deux pistes.", status.lag.abs()), LISTEN)
            };
            ui.label(RichText::new(text).color(color));
            slider(ui, "Début de restauration", &params.stem_start, setter);
            slider(ui, "Récupération", &params.stem_recovery, setter);
        }
        StemChoice::Blind => {
            slider(ui, "Début de restauration", &params.stem_start, setter);
            slider(ui, "Pente cible", &params.stem_slope, setter);
            slider(ui, "Remontée maximale", &params.stem_boost, setter);
            slider(ui, "Aigus recréés", &params.stem_synth, setter);
            ui.label(RichText::new("Les aigus recréés sont inventés : à doser à l'oreille.").color(LABEL));
        }
    }
}

fn resonance_list(ui: &mut egui::Ui, view: &View) {
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("Résonances retirées : {}", view.resonances.len())).strong());
        if view.resonances.is_empty() {
            ui.label(RichText::new("aucune pour l'instant").color(LABEL));
        }
        for r in &view.resonances {
            let alpha = (80.0 + 175.0 * r.weight) as u8;
            let color = Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), alpha);
            ui.label(RichText::new(format!("{}  {} dB", hz_label(r.hz), fr(r.level_db, 0))).color(color));
            ui.label(RichText::new("·").color(LABEL));
        }
    });
}

/// Remplit les colonnes vides par interpolation linéaire.
fn fill_gaps(cols: &mut [f32]) {
    let mut last: Option<usize> = None;
    for i in 0..cols.len() {
        if cols[i].is_finite() {
            if let Some(j) = last {
                for g in j + 1..i {
                    let t = (g - j) as f32 / (i - j) as f32;
                    cols[g] = cols[j] + (cols[i] - cols[j]) * t;
                }
            }
            last = Some(i);
        }
    }
}

fn analyzer(ui: &mut egui::Ui, view: &View, params: &ScalpelParams, shared: &Shared) {
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, BG);
    let plot = Rect::from_min_max(rect.min + Vec2::new(36.0, 22.0), rect.max - Vec2::new(8.0, 20.0));

    let decades = (MAX_HZ / MIN_HZ).log10();
    let fx = |hz: f32| plot.left() + (hz / MIN_HZ).log10() / decades * plot.width();
    let fy = |db: f32| {
        let t = ((TOP_DB - db) / (TOP_DB - BOTTOM_DB)).clamp(0.0, 1.0);
        plot.top() + t * plot.height()
    };
    let small = FontId::proportional(11.0);

    // Grille.
    for hz in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0] {
        let x = fx(hz);
        painter.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.0_f32, GRID));
        let text = if hz >= 1000.0 { format!("{}k", hz / 1000.0) } else { format!("{hz}") };
        painter.text(Pos2::new(x, plot.bottom() + 3.0), Align2::CENTER_TOP, text, small.clone(), LABEL);
    }
    let mut db = TOP_DB;
    while db >= BOTTOM_DB {
        let y = fy(db);
        painter.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], Stroke::new(1.0_f32, GRID));
        painter.text(Pos2::new(plot.left() - 4.0, y), Align2::RIGHT_CENTER, format!("{db}"), small.clone(), LABEL);
        db -= 20.0;
    }

    // Zones hors de la plage traitée.
    let shade = Color32::from_rgba_unmultiplied(0, 0, 0, 110);
    let lo = fx(params.res_min_hz.value().max(MIN_HZ));
    let hi = fx(params.res_max_hz.value().min(MAX_HZ));
    painter.rect_filled(Rect::from_min_max(plot.min, Pos2::new(lo, plot.bottom())), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(hi, plot.top()), plot.max), 0.0, shade);

    // Spectres, ramenés à une valeur par colonne de pixels.
    let width = plot.width().max(1.0) as usize;
    if view.fft_size > 0 && !view.input.is_empty() {
        let bin_hz = view.sample_rate / view.fft_size as f32;
        let mut cols = [vec![f32::NEG_INFINITY; width], vec![f32::NEG_INFINITY; width], vec![f32::NEG_INFINITY; width]];
        for k in 1..view.input.len() {
            let hz = k as f32 * bin_hz;
            if !(MIN_HZ..=MAX_HZ).contains(&hz) {
                continue;
            }
            let c = ((fx(hz) - plot.left()) as usize).min(width - 1);
            for (col, src) in cols.iter_mut().zip([&view.input, &view.removed, &view.output]) {
                col[c] = col[c].max(src[k]);
            }
        }
        for col in cols.iter_mut() {
            fill_gaps(col);
        }
        let removed_fill = Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 170);
        for c in 0..width {
            let x = plot.left() + c as f32 + 0.5;
            if cols[0][c].is_finite() {
                painter.line_segment([Pos2::new(x, fy(cols[0][c])), Pos2::new(x, plot.bottom())], Stroke::new(1.0_f32, INPUT));
            }
            if cols[1][c].is_finite() && cols[1][c] > BOTTOM_DB {
                painter.line_segment(
                    [Pos2::new(x, fy(cols[1][c])), Pos2::new(x, plot.bottom())],
                    Stroke::new(1.0_f32, removed_fill),
                );
            }
        }
        let line: Vec<Pos2> = (0..width)
            .filter(|&c| cols[2][c].is_finite())
            .map(|c| Pos2::new(plot.left() + c as f32 + 0.5, fy(cols[2][c])))
            .collect();
        painter.add(egui::Shape::line(line, Stroke::new(1.2_f32, OUTPUT)));
    }

    // Repères des résonances soustraites.
    let mut last_label_x = f32::NEG_INFINITY;
    let mut by_freq: Vec<&ResonanceInfo> = view.resonances.iter().collect();
    by_freq.sort_by(|a, b| a.hz.total_cmp(&b.hz));
    for r in by_freq {
        if !(MIN_HZ..=MAX_HZ).contains(&r.hz) {
            continue;
        }
        let x = fx(r.hz);
        let alpha = (60.0 + 195.0 * r.weight) as u8;
        let color = Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), alpha);
        let y = fy(r.level_db);
        painter.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, y)], Stroke::new(1.0_f32, color));
        painter.circle_stroke(Pos2::new(x, y), 4.0, Stroke::new(1.5_f32, color));
        if x - last_label_x > 46.0 {
            painter.text(Pos2::new(x, plot.top() - 2.0), Align2::CENTER_BOTTOM, hz_label(r.hz), small.clone(), color);
            last_label_x = x;
        }
    }

    if view.is_silent() {
        painter.text(
            plot.center(),
            Align2::CENTER_CENTER,
            "En attente de signal : lance la lecture",
            FontId::proportional(16.0),
            LABEL,
        );
    }

    // Légende et témoins.
    let legend = [("Entrée", INPUT), ("Retiré / ajouté", ACCENT), ("Sortie", OUTPUT)];
    let mut x = plot.right() - 250.0;
    for (name, color) in legend {
        painter.rect_filled(Rect::from_min_size(Pos2::new(x, plot.top() + 8.0), Vec2::new(10.0, 10.0)), 2.0, color);
        painter.text(Pos2::new(x + 14.0, plot.top() + 13.0), Align2::LEFT_CENTER, name, small.clone(), OUTPUT);
        x += 66.0;
    }
    let banner = if shared.is_cancel_test() {
        Some(("TEST D'ANNULATION", TEST))
    } else if shared.is_listening() {
        Some(("ÉCOUTE DE CE QUI EST RETIRÉ", LISTEN))
    } else {
        None
    };
    if let Some((text, color)) = banner {
        let r = Rect::from_min_size(Pos2::new(plot.left() + 8.0, plot.top() + 6.0), Vec2::new(250.0, 24.0));
        painter.rect_filled(r, 4.0, color);
        painter.text(r.center(), Align2::CENTER_CENTER, text, FontId::proportional(13.0), Color32::WHITE);
    }
}
