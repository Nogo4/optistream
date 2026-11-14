//! Main window: resource type, source folder, settings, output, analysis, comparison, report.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, TextureHandle, Ui, Vec2};
use egui_extras::{Column, TableBuilder};
use egui_material_icons::icons::*;
use optistream_core::pipeline::FIVEM_WARN_BYTES;
use optistream_core::{
    mib, Analysis, AnalyzedFile, FileReport, FileStatus, Issue, Method, Output, ParamKind, PreviewContent,
    PreviewImage, PreviewItem, Report, Settings, Severity, Value,
};
use serde::{Deserialize, Serialize};

use crate::jobs::{self, Job, JobResult};
use crate::render::{self, Bounds, View};
use crate::theme::{self, ACCENT, BG, BORDER, CARD, ERR, TEXT, WARN, WEAK};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum OutputMode {
    Export,
    InPlace,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Analysis,
    Comparison,
    Report,
}

/// What is remembered between sessions.
#[derive(Serialize, Deserialize)]
struct Saved {
    method: String,
    source: Option<PathBuf>,
    export_dir: Option<PathBuf>,
    output: OutputMode,
    overwrite: bool,
    settings: Settings,
}

/// What is still wrong with a file after its optimisation.
struct Remaining {
    /// Warnings and errors found by re-analysing the written file.
    issues: Vec<Issue>,
    /// Warnings reported by the optimiser itself (tolerance reached, textures skipped...).
    notes: Vec<String>,
    worst: Severity,
}

fn remaining_entry(f: &FileReport, checked: Option<&AnalyzedFile>) -> Option<Remaining> {
    let mut issues: Vec<Issue> = Vec::new();
    if let FileStatus::Error(e) = &f.status {
        issues.push(Issue::error(format!("échec de l'optimisation : {e}")));
    }
    match checked.map(|c| &c.analysis) {
        Some(Ok(a)) => issues.extend(a.issues.iter().filter(|i| i.severity >= Severity::Warning).cloned()),
        Some(Err(e)) => issues.push(Issue::error(format!("fichier produit illisible : {e}"))),
        None => {}
    }
    let notes = f.warnings.clone();
    if issues.is_empty() && notes.is_empty() {
        return None;
    }
    let worst = issues.iter().map(|i| i.severity).max().unwrap_or(Severity::Warning);
    Some(Remaining { issues, notes, worst })
}

/// Folds a partial run into the previous report, keeping each file's original "before".
fn merge_report(old: &mut Report, new: Report, selection: &HashSet<PathBuf>) {
    // Files of this run are either written now or listed again in the new run.
    old.not_written.retain(|p| !selection.contains(p));
    old.not_written.extend(new.not_written);
    old.not_written.sort();
    old.copied_files += new.copied_files;
    for mut f in new.files {
        match old.files.iter_mut().find(|x| x.rel == f.rel) {
            Some(slot) => {
                f.before = slot.before;
                *slot = f;
            }
            None => old.files.push(f),
        }
    }
    old.seconds += new.seconds;
    old.cancelled = new.cancelled;
    old.resource_issues = new.resource_issues;
}

struct LoadedPreview {
    item: PreviewItem,
    before: Option<TextureHandle>,
    after: Option<TextureHandle>,
    /// Mesh comparison: shared framing and the view the textures were rendered for.
    frame: Option<Bounds>,
    rendered: Option<(f32, bool)>,
}

pub struct App {
    methods: Vec<Arc<dyn Method>>,
    method: usize,
    source: Option<PathBuf>,
    export_dir: Option<PathBuf>,
    output: OutputMode,
    overwrite: bool,
    settings: Settings,

    job: Option<Job>,
    error: Option<String>,
    status: String,
    tab: Tab,

    analysis: Option<Analysis>,
    analysis_settings: Option<Settings>,
    only_problems: bool,
    filter: String,
    selected: Option<usize>,
    /// Files ticked for a partial optimisation.
    checked: HashSet<PathBuf>,
    /// Optimised files that still carry warnings, from the post-optimisation verification.
    remaining: BTreeMap<PathBuf, Remaining>,

    preview_file: Option<PathBuf>,
    previews: Vec<LoadedPreview>,
    zoom: f32,
    yaw: f32,
    wireframe: bool,
    show_alpha: bool,

    report: Option<Report>,
    /// Pending in-place optimisation awaiting confirmation (`Some(None)` = every file).
    confirm_in_place: Option<Option<HashSet<PathBuf>>>,
    /// The window was asked to close while a job runs.
    confirm_close: bool,
    /// Closing confirmed by the user despite a running job.
    allow_close: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let methods = registered_methods();
        let mut app = App {
            settings: methods[0].default_settings(),
            methods,
            method: 0,
            source: None,
            export_dir: None,
            output: OutputMode::Export,
            overwrite: false,
            job: None,
            error: None,
            status: "Prêt".to_owned(),
            tab: Tab::Analysis,
            analysis: None,
            analysis_settings: None,
            only_problems: true,
            filter: String::new(),
            selected: None,
            preview_file: None,
            previews: Vec::new(),
            zoom: 1.0,
            yaw: 0.0,
            wireframe: true,
            show_alpha: false,
            report: None,
            confirm_in_place: None,
            checked: HashSet::new(),
            remaining: BTreeMap::new(),
            confirm_close: false,
            allow_close: false,
        };
        if let Some(saved) = cc.storage.and_then(|s| eframe::get_value::<Saved>(s, eframe::APP_KEY)) {
            if let Some(i) = app.methods.iter().position(|m| m.id() == saved.method) {
                app.method = i;
            }
            app.source = saved.source;
            app.export_dir = saved.export_dir;
            app.output = saved.output;
            app.overwrite = saved.overwrite;
            // Settings added since the last save take their default-preset value.
            let mut settings = app.methods[app.method].default_settings();
            settings.0.extend(saved.settings.0);
            app.settings = settings;
        }
        // A folder passed on the command line (or dropped on the exe) is analysed right away.
        if let Some(dir) = std::env::args().nth(1).map(PathBuf::from).filter(|p| p.is_dir()) {
            app.source = Some(dir);
            app.start_analysis(&cc.egui_ctx);
        }
        app
    }

    fn method(&self) -> Arc<dyn Method> {
        self.methods[self.method].clone()
    }

    fn busy(&self) -> bool {
        self.job.is_some()
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.job else { return };
        let Some(result) = job.take() else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        };
        self.job = None;
        match result {
            Ok(JobResult::Analysis { analysis, partial }) => {
                self.status = format!("Analyse terminée : {} fichier(s)", analysis.files.len());
                if partial {
                    self.merge_analysis(analysis);
                } else {
                    self.analysis = Some(analysis);
                    self.selected = None;
                    self.checked.clear();
                }
                self.tab = Tab::Analysis;
            }
            Ok(JobResult::Report { report, verification, selection }) => {
                let partial = selection.is_some();
                let t = report.totals();
                self.status = format!(
                    "Optimisation terminée : {} / {} fichier(s) optimisé(s), mémoire physique {} → {}",
                    t.optimized,
                    t.processed,
                    mib(t.graphics_before),
                    mib(t.graphics_after)
                );
                // What is left to fix in the files that were just written.
                let same_target = self.report.as_ref().is_some_and(|r| r.destination == report.destination);
                if !(partial && same_target) {
                    self.remaining.clear();
                }
                for f in &report.files {
                    let checked = verification.as_ref().and_then(|v| v.files.iter().find(|x| x.rel == f.rel));
                    match remaining_entry(f, checked) {
                        Some(r) => {
                            self.remaining.insert(f.rel.clone(), r);
                        }
                        None => {
                            self.remaining.remove(&f.rel);
                        }
                    }
                }
                // In place, the source is the destination: keep the analysis table truthful.
                if report.in_place {
                    if let Some(v) = verification {
                        self.merge_analysis(v);
                    }
                }
                if partial {
                    self.checked.clear();
                }
                match (&mut self.report, &selection, same_target) {
                    (Some(old), Some(sel), true) => merge_report(old, report, sel),
                    _ => self.report = Some(report),
                }
                self.tab = Tab::Report;
            }
            Ok(JobResult::Preview(file, items)) => {
                self.status = "Comparaison prête".to_owned();
                self.preview_file = Some(file);
                self.previews = items
                    .into_iter()
                    .enumerate()
                    .map(|(i, item)| load_preview(ctx, i, item, self.show_alpha))
                    .collect();
                self.tab = Tab::Comparison;
            }
            Err(e) => {
                self.status = "Erreur".to_owned();
                self.error = Some(e);
            }
        }
    }

    /// Replaces the analysed entries of a partial analysis, keeping the others.
    fn merge_analysis(&mut self, fresh: Analysis) {
        let Some(a) = &mut self.analysis else {
            self.analysis = Some(fresh);
            return;
        };
        for f in fresh.files {
            match a.files.iter_mut().find(|x| x.rel == f.rel) {
                Some(slot) => *slot = f,
                None => a.files.push(f),
            }
        }
        a.resource_issues = fresh.resource_issues;
    }

    fn start_analysis(&mut self, ctx: &egui::Context) {
        if let Some(src) = self.source.clone() {
            self.analysis_settings = Some(self.settings.clone());
            self.job = Some(jobs::analyze(ctx.clone(), self.method(), src, self.settings.clone(), None));
        }
    }

    /// Optimises everything (`None`) or the given files, asking first when working in place.
    fn request_optimize(&mut self, ctx: &egui::Context, only: Option<HashSet<PathBuf>>) {
        if self.output == OutputMode::InPlace {
            self.confirm_in_place = Some(only);
        } else {
            self.start_optimize(ctx, only);
        }
    }

    fn start_optimize(&mut self, ctx: &egui::Context, only: Option<HashSet<PathBuf>>) {
        let Some(src) = self.source.clone() else { return };
        let output = match self.output {
            OutputMode::InPlace => Output::InPlace,
            OutputMode::Export => match &self.export_dir {
                Some(d) => Output::Export { dir: d.clone(), overwrite: self.overwrite },
                None => {
                    self.error = Some("Choisis un dossier d'export.".into());
                    return;
                }
            },
        };
        self.job = Some(jobs::optimize(ctx.clone(), self.method(), src, self.settings.clone(), output, only));
    }

    fn start_preview(&mut self, ctx: &egui::Context, rel: PathBuf) {
        let Some(src) = self.source.clone() else { return };
        let file = match optistream_core::contexts(&src) {
            Ok(all) => all.into_iter().find(|c| c.rel == rel),
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                return;
            }
        };
        if let Some(file) = file {
            self.job = Some(jobs::preview(ctx.clone(), self.method(), file, self.settings.clone()));
        }
    }

    // ------------------------------------------------------------ top / bottom bars

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(theme::icon(ICON_SPEED, 26.0, ACCENT));
            ui.label(RichText::new("OptiStream").size(20.0).strong().color(TEXT));
            ui.label(RichText::new("Optimisation des streams FiveM").color(WEAK));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("GTA V Legacy").small().color(WEAK));
                ui.label(theme::icon(ICON_SPORTS_ESPORTS, 16.0, WEAK));
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            match &self.job {
                Some(job) => {
                    let p = &job.progress;
                    let (done, total) = (p.done.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed));
                    ui.add(egui::Spinner::new().size(14.0).color(ACCENT));
                    ui.label(RichText::new(format!("{} en cours", job.label)).strong());
                    ui.add(
                        egui::ProgressBar::new(p.fraction())
                            .desired_width(260.0)
                            .fill(ACCENT)
                            .text(RichText::new(format!("{done} / {total}")).small()),
                    );
                    if let Ok(cur) = p.current.lock() {
                        ui.add(egui::Label::new(RichText::new(cur.as_str()).small().color(WEAK)).truncate());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(theme::label(ICON_STOP_CIRCLE, "Annuler")).clicked() {
                            p.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                }
                None => {
                    ui.label(theme::icon(ICON_TASK_ALT, 15.0, ACCENT));
                    ui.label(RichText::new(&self.status).small().color(WEAK));
                }
            }
        });
    }

    // ------------------------------------------------------------ side panel

    fn side_panel(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let busy = self.busy();
        ui.add_space(10.0);
        // Steps scroll; the action buttons stay pinned at the bottom.
        let actions_height = 96.0;
        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - actions_height)
            .auto_shrink([false, false])
            .show(ui, |ui| self.steps(ui, busy));
        ui.add_space(8.0);
        self.actions(ui, &ctx, busy);
    }

    fn steps(&mut self, ui: &mut Ui, busy: bool) {
        ui.add_enabled_ui(!busy, |ui| {
            theme::card(ui, 1, ICON_APPAREL, "Ressource", |ui| {
                ui.label(RichText::new("Type").small().color(WEAK));
                let current = self.methods[self.method].label();
                let mut changed = false;
                egui::ComboBox::from_id_salt("method")
                    .selected_text(current)
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (i, m) in self.methods.iter().enumerate() {
                            changed |= ui.selectable_value(&mut self.method, i, m.label()).changed();
                        }
                    });
                if changed {
                    self.settings = self.method().default_settings();
                    self.analysis = None;
                }
                ui.label(RichText::new(self.methods[self.method].description()).small().color(WEAK));
                ui.add_space(4.0);
                ui.label(RichText::new("Dossier de la ressource (ou d'une catégorie [...])").small().color(WEAK));
                theme::path_field(ui, &mut self.source, "Choisir le dossier de la ressource");
            });
            ui.add_space(8.0);
            theme::card(ui, 2, ICON_TUNE, "Réglages", |ui| self.settings_ui(ui));
            ui.add_space(8.0);
            theme::card(ui, 3, ICON_OUTPUT, "Sortie", |ui| self.output_ui(ui));
        });
    }

    fn actions(&mut self, ui: &mut Ui, ctx: &egui::Context, busy: bool) {
        let ready = self.source.is_some() && !busy;
        let half = (ui.available_width() - 8.0) / 2.0;
        ui.horizontal(|ui| {
            if theme::secondary_button(ui, ready, ICON_SEARCH, "Analyser", half).clicked() {
                self.start_analysis(ctx);
            }
            if theme::secondary_button(ui, ready && self.selected.is_some(), ICON_COMPARE, "Comparer", half)
                .on_hover_text("Comparer avant / après le fichier sélectionné dans l'analyse")
                .clicked()
            {
                if let Some(rel) = self.selected_rel() {
                    self.start_preview(ctx, rel);
                }
            }
        });
        ui.add_space(4.0);
        // Ticked files turn the main action into a partial optimisation.
        let text = match self.checked.len() {
            0 => "Optimiser toute la ressource".to_owned(),
            n => format!("Optimiser la sélection ({n})"),
        };
        if theme::primary_button(ui, ready, ICON_BOLT, &text).clicked() {
            let only = (!self.checked.is_empty()).then(|| self.checked.clone());
            self.request_optimize(ctx, only);
        }
        ui.add_space(10.0);
    }

    fn selected_rel(&self) -> Option<PathBuf> {
        let a = self.analysis.as_ref()?;
        Some(a.files.get(self.selected?)?.rel.clone())
    }

    fn output_ui(&mut self, ui: &mut Ui) {
        let option = |ui: &mut Ui, current: &mut OutputMode, value: OutputMode, ic, title: &str, hint: &str| {
            let selected = *current == value;
            let stroke = if selected { Stroke::new(1.5, ACCENT) } else { Stroke::new(1.0, BORDER) };
            let resp = egui::Frame::new()
                .fill(if selected { ACCENT.gamma_multiply(0.08) } else { BG })
                .stroke(stroke)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(theme::icon(ic, 20.0, if selected { ACCENT } else { WEAK }));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(title).strong());
                            ui.label(RichText::new(hint).small().color(WEAK));
                        });
                    });
                })
                .response
                .interact(egui::Sense::click());
            if resp.clicked() {
                *current = value;
            }
        };
        option(ui, &mut self.output, OutputMode::Export, ICON_DRIVE_FILE_MOVE, "Exporter", "Nouvelle ressource optimisée, l'originale reste intacte");
        if self.output == OutputMode::Export {
            ui.add_space(2.0);
            theme::path_field(ui, &mut self.export_dir, "Choisir le dossier d'export");
            if let (Some(src), Some(dir)) = (&self.source, &self.export_dir) {
                if let Some(name) = src.file_name() {
                    ui.horizontal(|ui| {
                        ui.label(theme::icon(ICON_ARROW_FORWARD, 13.0, WEAK));
                        ui.add(egui::Label::new(RichText::new(dir.join(name).display().to_string()).small().color(WEAK)).truncate());
                    });
                }
            }
            ui.checkbox(&mut self.overwrite, "Écraser si le dossier existe déjà");
            ui.add_space(4.0);
        }
        option(ui, &mut self.output, OutputMode::InPlace, ICON_EDIT_DOCUMENT, "Modifier sur place", "Remplace les fichiers du dossier source");
        if self.output == OutputMode::InPlace {
            theme::icon_line(ui, ICON_WARNING, WARN, "Sans sauvegarde : les fichiers d'origine sont remplacés.");
        }
    }

    fn settings_ui(&mut self, ui: &mut Ui) {
        let method = self.method();
        let presets = method.presets();
        let active = presets.iter().find(|p| p.values == self.settings).map(|p| p.id);

        // Segmented preset selector.
        let width = (ui.available_width() - 8.0 * (presets.len() as f32 - 1.0)) / presets.len() as f32;
        ui.horizontal(|ui| {
            for p in &presets {
                let on = active == Some(p.id);
                let b = egui::Button::new(RichText::new(p.label).color(if on { theme::ACCENT_TEXT } else { TEXT }).strong())
                    .fill(if on { ACCENT } else { BG })
                    .stroke(Stroke::new(1.0, if on { ACCENT } else { BORDER }))
                    .corner_radius(CornerRadius::same(6))
                    .min_size(Vec2::new(width, 28.0));
                if ui.add(b).on_hover_text(p.description).clicked() {
                    self.settings = p.values.clone();
                }
            }
        });
        match presets.iter().find(|p| Some(p.id) == active) {
            Some(p) => {
                ui.label(RichText::new(p.description).small().color(WEAK));
            }
            None => {
                ui.horizontal(|ui| {
                    ui.label(theme::icon(ICON_TUNE, 14.0, WARN));
                    ui.label(RichText::new("Réglages personnalisés").small().color(WARN));
                    if ui.small_button(theme::label(ICON_RESTART_ALT, "Par défaut")).clicked() {
                        self.settings = method.default_settings();
                    }
                });
            }
        }
        ui.add_space(4.0);

        let params = method.params();
        let mut groups: Vec<&str> = Vec::new();
        for p in &params {
            if !groups.contains(&p.group) {
                groups.push(p.group);
            }
        }
        for group in groups {
            let ic = match group {
                "Textures" => ICON_TEXTURE,
                "Maillages" => ICON_VIEW_IN_AR,
                _ => ICON_TUNE,
            };
            egui::CollapsingHeader::new(theme::label(ic, group)).id_salt(group).default_open(false).show(ui, |ui| {
                egui::Grid::new(group).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    for p in params.iter().filter(|p| p.group == group) {
                        ui.label(RichText::new(p.label).small()).on_hover_text(p.help);
                        match &p.kind {
                            ParamKind::Bool => {
                                let mut v = self.settings.bool(p.key, false);
                                if ui.checkbox(&mut v, "").on_hover_text(p.help).changed() {
                                    self.settings.set(p.key, Value::Bool(v));
                                }
                            }
                            ParamKind::Int { min, max, step, unit } => {
                                let mut v = self.settings.int(p.key, *min);
                                let w = egui::DragValue::new(&mut v)
                                    .range(*min..=*max)
                                    .speed(*step as f64 / 4.0)
                                    .suffix(format!(" {unit}"));
                                if ui.add(w).on_hover_text(p.help).changed() {
                                    self.settings.set(p.key, Value::Int(v));
                                }
                            }
                            ParamKind::Float { min, max, step, unit } => {
                                let mut v = self.settings.float(p.key, *min);
                                let w = egui::DragValue::new(&mut v)
                                    .range(*min..=*max)
                                    .speed(*step)
                                    .fixed_decimals(2)
                                    .suffix(format!(" {unit}"));
                                if ui.add(w).on_hover_text(p.help).changed() {
                                    self.settings.set(p.key, Value::Float(v));
                                }
                            }
                            ParamKind::Choice(options) => {
                                let mut v = self.settings.int(p.key, options[0].0);
                                let text = options.iter().find(|o| o.0 == v).map(|o| o.1).unwrap_or("?");
                                egui::ComboBox::from_id_salt(p.key).selected_text(text).width(90.0).show_ui(ui, |ui| {
                                    for (val, label) in options {
                                        if ui.selectable_value(&mut v, *val, *label).changed() {
                                            self.settings.set(p.key, Value::Int(v));
                                        }
                                    }
                                });
                            }
                        }
                        ui.end_row();
                    }
                });
            });
        }
    }

    // ------------------------------------------------------------ tabs

    fn tab_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for (tab, ic, title) in [
                (Tab::Analysis, ICON_TABLE_CHART, "Analyse"),
                (Tab::Comparison, ICON_COMPARE, "Comparaison"),
                (Tab::Report, ICON_DESCRIPTION, "Rapport"),
            ] {
                let on = self.tab == tab;
                let text = RichText::new(theme::label(ic, title)).size(15.0).color(if on { ACCENT } else { WEAK });
                let resp = ui.add(egui::Button::new(text).frame(false).min_size(Vec2::new(0.0, 30.0)));
                if on {
                    let r = resp.rect;
                    ui.painter().line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(2.0, ACCENT));
                }
                if resp.clicked() {
                    self.tab = tab;
                }
                ui.add_space(12.0);
            }
        });
        let r = ui.available_rect_before_wrap();
        ui.painter().line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, BORDER));
        ui.add_space(8.0);
    }

    fn analysis_tab(&mut self, ui: &mut Ui) {
        let Some(a) = &self.analysis else {
            theme::empty_state(
                ui,
                ICON_TABLE_CHART,
                "Aucune analyse",
                "Choisis le dossier de la ressource puis lance « Analyser ».",
            );
            return;
        };
        let warned = a.files.iter().filter(|f| f.size.fivem_warning()).count();
        let with_issues = a
            .files
            .iter()
            .filter(|f| f.analysis.as_ref().map(|x| !x.issues.is_empty()).unwrap_or(true))
            .count();
        let gfx: u64 = a.files.iter().map(|f| f.size.graphics).sum();
        ui.horizontal_top(|ui| {
            theme::stat(ui, ICON_FOLDER, "Fichiers", a.files.len().to_string(), WEAK);
            theme::stat(ui, ICON_CHECKLIST, "À optimiser", with_issues.to_string(), WARN);
            theme::stat(ui, ICON_WARNING, "> 16 Mio (FiveM)", warned.to_string(), if warned > 0 { ERR } else { ACCENT });
            theme::stat(ui, ICON_MEMORY, "Mémoire physique", mib(gfx), ACCENT);
        });
        ui.add_space(6.0);
        if self.analysis_settings.as_ref() != Some(&self.settings) {
            theme::icon_line(ui, ICON_INFO, WARN, "Les réglages ont changé depuis l'analyse : relance « Analyser » pour mettre à jour.");
        }
        for i in &a.resource_issues {
            let (ic, c) = theme::severity_icon(i.severity);
            theme::icon_line(ui, ic, c, &i.text);
        }
        ui.add_space(4.0);

        let filter = self.filter.to_lowercase();
        let rows: Vec<usize> = a
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                let problems = f.analysis.as_ref().map(|x| !x.issues.is_empty()).unwrap_or(true);
                (!self.only_problems || problems)
                    && (filter.is_empty() || f.rel.to_string_lossy().to_lowercase().contains(&filter))
            })
            .map(|(i, _)| i)
            .collect();

        let mut optimize_now: Option<HashSet<PathBuf>> = None;
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.only_problems, "Seulement les fichiers à optimiser");
            ui.add_space(12.0);
            ui.label(theme::icon(ICON_FILTER_LIST, 18.0, WEAK));
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filtrer par nom…").desired_width(240.0));
            if !self.checked.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let b = egui::Button::new(RichText::new(theme::label(ICON_BOLT, &format!("Optimiser la sélection ({})", self.checked.len()))).color(theme::ACCENT_TEXT).strong())
                        .fill(ACCENT);
                    if ui.add_enabled(self.job.is_none(), b).clicked() {
                        optimize_now = Some(self.checked.clone());
                    }
                    if ui.button(theme::label(ICON_CLOSE, "Vider")).on_hover_text("Décocher tous les fichiers").clicked() {
                        self.checked.clear();
                    }
                });
            }
        });
        ui.add_space(4.0);

        let mut clicked = None;
        let all_checked = !rows.is_empty() && rows.iter().all(|&i| self.checked.contains(&a.files[i].rel));
        let mut toggle_all = None;
        let detail_height = if self.selected.is_some() { 190.0 } else { 0.0 };
        let table_height = ui.available_height() - detail_height;
        TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .max_scroll_height(table_height)
            .column(Column::exact(26.0))
            .column(Column::exact(28.0))
            .column(Column::remainder().at_least(260.0).clip(true))
            .column(Column::exact(44.0))
            .column(Column::exact(84.0))
            .column(Column::exact(84.0))
            .column(Column::initial(240.0).clip(true))
            .header(24.0, |mut h| {
                h.col(|ui| {
                    let mut v = all_checked;
                    if ui.checkbox(&mut v, "").on_hover_text("Cocher / décocher les fichiers affichés").changed() {
                        toggle_all = Some(v);
                    }
                });
                for t in ["", "Fichier", "Type", "Physique", "Virtuelle", "Résumé"] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).small().strong().color(WEAK));
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, rows.len(), |mut row| {
                    let idx = rows[row.index()];
                    let f = &a.files[idx];
                    row.set_selected(self.selected == Some(idx));
                    let (kind, summary, worst, count) = match &f.analysis {
                        Ok(x) => (x.kind.clone(), x.summary.clone(), x.issues.iter().map(|i| i.severity).max(), x.issues.len()),
                        Err(e) => ("?".into(), e.clone(), Some(optistream_core::Severity::Error), 1),
                    };
                    row.col(|ui| {
                        let mut v = self.checked.contains(&f.rel);
                        if ui.checkbox(&mut v, "").changed() {
                            if v {
                                self.checked.insert(f.rel.clone());
                            } else {
                                self.checked.remove(&f.rel);
                            }
                        }
                    });
                    row.col(|ui| {
                        let (ic, c) = match worst {
                            Some(s) => theme::severity_icon(s),
                            None => (ICON_CHECK_CIRCLE, ACCENT),
                        };
                        ui.label(theme::icon(ic, 17.0, c)).on_hover_text(match count {
                            0 => "Conforme".to_owned(),
                            n => format!("{n} point(s) à corriger"),
                        });
                    });
                    row.col(|ui| {
                        ui.label(f.rel.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
                            .on_hover_text(f.rel.display().to_string());
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(kind).small().color(WEAK));
                    });
                    row.col(|ui| {
                        let c = if f.size.graphics > FIVEM_WARN_BYTES { ERR } else { TEXT };
                        ui.label(RichText::new(mib(f.size.graphics)).color(c));
                    });
                    row.col(|ui| {
                        let c = if f.size.system > FIVEM_WARN_BYTES { WARN } else { TEXT };
                        ui.label(RichText::new(mib(f.size.system)).color(c));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(summary).small().color(WEAK));
                    });
                    if row.response().clicked() {
                        clicked = Some(idx);
                    }
                });
            });
        if let Some(i) = clicked {
            self.selected = Some(i);
        }
        if let Some(v) = toggle_all {
            for &i in &rows {
                if v {
                    self.checked.insert(a.files[i].rel.clone());
                } else {
                    self.checked.remove(&a.files[i].rel);
                }
            }
        }

        let mut compare = None;
        if let Some(sel) = self.selected {
            if let Some(f) = a.files.get(sel) {
                ui.add_space(6.0);
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(self.job.is_none(), egui::Button::new(theme::label(ICON_COMPARE, "Comparer avant / après")))
                                .clicked()
                            {
                                compare = Some(f.rel.clone());
                            }
                            if ui
                                .add_enabled(self.job.is_none(), egui::Button::new(theme::label(ICON_BOLT, "Optimiser ce fichier")))
                                .clicked()
                            {
                                optimize_now = Some(HashSet::from([f.rel.clone()]));
                            }
                            ui.add(egui::Label::new(RichText::new(f.rel.display().to_string()).strong()).truncate());
                        });
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().id_salt("issues").max_height(120.0).show(ui, |ui| match &f.analysis {
                            Ok(x) if x.issues.is_empty() => theme::icon_line(ui, ICON_CHECK_CIRCLE, ACCENT, "Rien à signaler."),
                            Ok(x) => {
                                for i in &x.issues {
                                    let (ic, c) = theme::severity_icon(i.severity);
                                    theme::icon_line(ui, ic, c, &i.text);
                                }
                            }
                            Err(e) => theme::icon_line(ui, ICON_ERROR, ERR, e),
                        });
                    });
            }
        }
        let ctx = ui.ctx().clone();
        if let Some(rel) = compare {
            self.start_preview(&ctx, rel);
        }
        if let Some(only) = optimize_now {
            self.request_optimize(&ctx, Some(only));
        }
    }

    fn comparison_tab(&mut self, ui: &mut Ui) {
        let Some(file) = &self.preview_file else {
            theme::empty_state(
                ui,
                ICON_COMPARE,
                "Aucune comparaison",
                "Sélectionne un fichier dans l'analyse puis « Comparer avant / après ».",
            );
            return;
        };
        let mut optimize_now = None;
        let mut tick = None;
        ui.horizontal(|ui| {
            ui.label(theme::icon(ICON_DEPLOYED_CODE, 18.0, ACCENT));
            ui.add(egui::Label::new(RichText::new(file.display().to_string()).strong()).truncate());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let b = egui::Button::new(RichText::new(theme::label(ICON_BOLT, "Optimiser ce fichier")).color(theme::ACCENT_TEXT).strong())
                    .fill(ACCENT);
                if ui.add_enabled(self.job.is_none(), b).on_hover_text("Applique les réglages actuels à ce seul fichier (selon la sortie choisie)").clicked() {
                    optimize_now = Some(HashSet::from([file.clone()]));
                }
                let mut v = self.checked.contains(file);
                if ui.checkbox(&mut v, "Ajouter à la sélection").changed() {
                    tick = Some(v);
                }
            });
        });
        let file = file.clone();
        if let Some(v) = tick {
            if v {
                self.checked.insert(file.clone());
            } else {
                self.checked.remove(&file);
            }
        }
        if let Some(only) = optimize_now {
            let ctx = ui.ctx().clone();
            self.request_optimize(&ctx, Some(only));
        }
        ui.horizontal(|ui| {
            ui.label(theme::icon(ICON_ZOOM_IN, 18.0, WEAK));
            ui.add(egui::Slider::new(&mut self.zoom, 0.25..=4.0).logarithmic(true).show_value(true));
            ui.add_space(10.0);
            ui.label(theme::icon(ICON_ROTATE_RIGHT, 18.0, WEAK));
            ui.add(egui::Slider::new(&mut self.yaw, -180.0..=180.0).suffix("°"));
            ui.add_space(10.0);
            ui.checkbox(&mut self.wireframe, theme::label(ICON_GRID_ON, "Fil de fer"));
            if ui
                .checkbox(&mut self.show_alpha, theme::label(ICON_OPACITY, "Alpha"))
                .on_hover_text("Sinon seules les couleurs RVB sont affichées (l'alpha des spéculaires = brillance).")
                .changed()
            {
                let ctx = ui.ctx().clone();
                let items: Vec<PreviewItem> = self.previews.drain(..).map(|p| p.item).collect();
                self.previews = items.into_iter().enumerate().map(|(i, item)| load_preview(&ctx, i, item, self.show_alpha)).collect();
            }
        });
        theme::icon_line(ui, ICON_INFO, WEAK, "Réglages actuels appliqués ; rien n'est écrit sur le disque.");
        ui.add_space(4.0);
        if self.previews.is_empty() {
            theme::empty_state(ui, ICON_COMPARE, "Rien à comparer", "Ce fichier ne contient ni maillage ni texture modifiable.");
            return;
        }

        let ctx = ui.ctx().clone();
        let view = View { yaw: self.yaw.to_radians(), wireframe: self.wireframe };
        let zoom = self.zoom;
        egui::ScrollArea::both().show(ui, |ui| {
            for (i, p) in self.previews.iter_mut().enumerate() {
                let is_mesh = matches!(p.item.content, PreviewContent::Mesh { .. });
                if let PreviewContent::Mesh { before, after } = &p.item.content {
                    if p.rendered != Some((view.yaw, view.wireframe)) {
                        let frame = *p.frame.get_or_insert_with(|| render::bounds(before));
                        p.before = Some(load_image(&ctx, &format!("mb{i}"), &render::render(before, frame, &view, 512)));
                        p.after = after.as_ref().map(|a| load_image(&ctx, &format!("ma{i}"), &render::render(a, frame, &view, 512)));
                        p.rendered = Some((view.yaw, view.wireframe));
                    }
                }
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(theme::icon(if is_mesh { ICON_VIEW_IN_AR } else { ICON_IMAGE }, 18.0, ACCENT));
                            ui.label(RichText::new(&p.item.title).strong());
                        });
                        ui.add_space(4.0);
                        let size = p.before.as_ref().map(|t| t.size_vec2() * zoom).unwrap_or_default();
                        // Same display width as "before": a smaller texture shows its loss of detail.
                        // A cropped one keeps its own proportions instead of being stretched.
                        let after_size = match p.after.as_ref().map(|t| t.size_vec2()) {
                            Some(a) if a.x > 0.0 && size.x > 0.0 && (a.y / a.x - size.y / size.x).abs() > 1e-3 => {
                                Vec2::new(size.x, size.x * a.y / a.x)
                            }
                            _ => size,
                        };
                        ui.horizontal_top(|ui| {
                            side(ui, "Avant", &p.item.before_desc, p.before.as_ref(), size, WEAK);
                            ui.add_space(8.0);
                            side(ui, "Après", &p.item.after_desc, p.after.as_ref(), after_size, ACCENT);
                        });
                    });
                ui.add_space(8.0);
            }
        });
    }

    fn report_tab(&mut self, ui: &mut Ui) {
        let Some(r) = &self.report else {
            theme::empty_state(ui, ICON_DESCRIPTION, "Aucun rapport", "Le rapport apparaît ici après « Optimiser ».");
            return;
        };
        let t = r.totals();
        ui.horizontal_top(|ui| {
            theme::stat(ui, ICON_TASK_ALT, "Optimisés", format!("{} / {}", t.optimized, t.processed), ACCENT);
            theme::stat(ui, ICON_ERROR, "Erreurs", t.errors.to_string(), if t.errors > 0 { ERR } else { WEAK });
            theme::stat(ui, ICON_MEMORY, "Mémoire physique", format!("{} → {}", mib(t.graphics_before), mib(t.graphics_after)), ACCENT);
            theme::stat(ui, ICON_HARD_DRIVE, "Disque", format!("{} → {}", mib(t.disk_before), mib(t.disk_after)), WEAK);
            theme::stat(ui, ICON_WARNING, "> 16 Mio", format!("{} → {}", t.warnings_before, t.warnings_after), WARN);
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(theme::icon(ICON_FOLDER, 16.0, WEAK));
            ui.add(egui::Label::new(RichText::new(r.destination.display().to_string()).color(WEAK)).truncate());
            if ui.button(theme::label(ICON_FOLDER_OPEN, "Ouvrir")).clicked() {
                let _ = std::process::Command::new("explorer").arg(&r.destination).spawn();
            }
            if ui.button(theme::label(ICON_SAVE, "Enregistrer le rapport")).clicked() {
                if let Some(path) = rfd::FileDialog::new().set_file_name("optistream-rapport.txt").save_file() {
                    if let Err(e) = std::fs::write(&path, r.to_text()) {
                        self.error = Some(e.to_string());
                    }
                }
            }
        });
        // A category folder ([oren-mappings]) holds many resources: totals for each one.
        let resources = r.by_resource();
        if resources.len() > 1 {
            ui.add_space(4.0);
            egui::CollapsingHeader::new(RichText::new(format!("Par ressource ({})", resources.len())).strong())
                .id_salt("report_by_resource")
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("report_by_resource_scroll").max_height(220.0).show(ui, |ui| {
                        egui::Grid::new("report_by_resource_grid").striped(true).num_columns(4).show(ui, |ui| {
                            for h in ["Ressource", "> 16 Mio", "Mémoire physique", "Optimisés"] {
                                ui.label(RichText::new(h).small().color(WEAK));
                            }
                            ui.end_row();
                            for (dir, t) in &resources {
                                let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "(racine)".to_owned());
                                ui.label(name).on_hover_text(dir.display().to_string());
                                let color = if t.warnings_after > 0 { WARN } else { ACCENT };
                                ui.label(RichText::new(format!("{} → {}", t.warnings_before, t.warnings_after)).color(color));
                                ui.label(format!("{} → {}", mib(t.graphics_before), mib(t.graphics_after)));
                                ui.label(format!("{} / {}", t.optimized, t.processed));
                                ui.end_row();
                            }
                        });
                    });
                });
        }
        let mut compare = None;
        let mut reoptimize: Option<HashSet<PathBuf>> = None;
        let idle = self.job.is_none();
        if !r.not_written.is_empty() {
            ui.add_space(4.0);
            egui::Frame::new()
                .fill(ERR.gamma_multiply(0.12))
                .stroke(Stroke::new(1.0, ERR))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(theme::icon(ICON_ERROR, 22.0, ERR));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(format!("Export incomplet : {} fichier(s) non écrit(s)", r.not_written.len())).strong().color(ERR));
                            ui.label(RichText::new("L'optimisation a été interrompue. Ne mets pas cette ressource en production telle quelle.").small());
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let b = egui::Button::new(RichText::new(theme::label(ICON_RESTART_ALT, "Compléter l'export")).color(Color32::WHITE).strong()).fill(ERR);
                            if ui.add_enabled(idle, b).on_hover_text("Écrit uniquement les fichiers manquants dans la destination").clicked() {
                                reoptimize = Some(r.not_written.iter().cloned().collect());
                            }
                        });
                    });
                });
        } else if r.cancelled {
            theme::icon_line(ui, ICON_WARNING, WARN, "Optimisation annulée : seule une partie des fichiers a été traitée.");
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Files still carrying warnings after the run, each fixable on its own.
            if self.remaining.is_empty() {
                theme::icon_line(ui, ICON_CHECK_CIRCLE, ACCENT, "Aucun avertissement restant sur les fichiers optimisés.");
            } else {
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0, WARN.gamma_multiply(0.5)))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(theme::icon(ICON_WARNING, 20.0, WARN));
                            ui.label(RichText::new(format!("À revoir ({})", self.remaining.len())).strong().size(15.0));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let all = egui::Button::new(RichText::new(theme::label(ICON_BOLT, "Tout ré-optimiser")).color(theme::ACCENT_TEXT).strong())
                                    .fill(ACCENT);
                                if ui.add_enabled(idle, all).on_hover_text("Relance ces fichiers avec les réglages actuels").clicked() {
                                    reoptimize = Some(self.remaining.keys().cloned().collect());
                                }
                            });
                        });
                        ui.label(
                            RichText::new("Ajuste les réglages à gauche (tolérance, tailles…), compare, puis relance fichier par fichier.")
                                .small()
                                .color(WEAK),
                        );
                        ui.add_space(6.0);
                        for (rel, rem) in &self.remaining {
                            let name = rel.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            let (ic, color) = theme::severity_icon(rem.worst);
                            egui::Frame::new()
                                .fill(BG)
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(Margin::symmetric(10, 8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.label(theme::icon(ic, 18.0, color));
                                        ui.add(egui::Label::new(RichText::new(&name).strong()).truncate())
                                            .on_hover_text(rel.display().to_string());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.add_enabled(idle, egui::Button::new(theme::label(ICON_BOLT, "Optimiser"))).clicked() {
                                                reoptimize = Some(HashSet::from([rel.clone()]));
                                            }
                                            if ui.add_enabled(idle, egui::Button::new(theme::label(ICON_COMPARE, "Comparer"))).clicked() {
                                                compare = Some(rel.clone());
                                            }
                                        });
                                    });
                                    for i in &rem.issues {
                                        let (ic, c) = theme::severity_icon(i.severity);
                                        theme::icon_line(ui, ic, c, &i.text);
                                    }
                                    for n in &rem.notes {
                                        theme::icon_line(ui, ICON_INFO, WEAK, n);
                                    }
                                });
                            ui.add_space(4.0);
                        }
                    });
            }
            ui.add_space(8.0);

            egui::CollapsingHeader::new(theme::label(ICON_CHECKLIST, "Détail des modifications"))
                .default_open(false)
                .show(ui, |ui| {
                    for f in &r.files {
                        let name = f.rel.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        match &f.status {
                            FileStatus::Unchanged if f.warnings.is_empty() => continue,
                            FileStatus::Error(e) => theme::icon_line(ui, ICON_ERROR, ERR, format!("{name} : {e}")),
                            _ => {
                                let ic = if f.warnings.is_empty() { ICON_CHECK_CIRCLE } else { ICON_WARNING };
                                egui::CollapsingHeader::new(format!(
                                    "{}  {name}   ·   {} → {}",
                                    ic.codepoint,
                                    mib(f.before.graphics),
                                    mib(f.after.graphics)
                                ))
                                .id_salt(&f.rel)
                                .show(ui, |ui| {
                                    for c in &f.changes {
                                        theme::icon_line(ui, ICON_ARROW_FORWARD, TEXT, c);
                                    }
                                    for w in &f.warnings {
                                        theme::icon_line(ui, ICON_WARNING, WARN, w);
                                    }
                                });
                            }
                        }
                    }
                });
        });
        let ctx = ui.ctx().clone();
        if let Some(rel) = compare {
            self.start_preview(&ctx, rel);
        }
        if let Some(only) = reoptimize {
            self.request_optimize(&ctx, Some(only));
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.confirm_close {
            egui::Window::new(theme::label(ICON_WARNING, "Une tâche est en cours"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Quitter maintenant laisse les fichiers restants non traités :");
                    ui.label("un export serait incomplet (il manquerait des fichiers).");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let quit = egui::Button::new(RichText::new("Quitter quand même").color(Color32::WHITE)).fill(ERR);
                        if ui.add(quit).clicked() {
                            if let Some(job) = &self.job {
                                job.progress.cancel.store(true, Ordering::Relaxed);
                            }
                            self.allow_close = true;
                            self.confirm_close = false;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button("Continuer la tâche").clicked() {
                            self.confirm_close = false;
                        }
                    });
                });
        }
        if let Some(only) = self.confirm_in_place.clone() {
            let mut decision = None;
            egui::Window::new(theme::label(ICON_WARNING, "Modifier le dossier directement ?"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    match &only {
                        None => ui.label("Tous les fichiers concernés seront remplacés, sans sauvegarde."),
                        Some(sel) if sel.len() == 1 => ui.label("Ce fichier sera remplacé, sans sauvegarde :"),
                        Some(sel) => ui.label(format!("Ces {} fichiers seront remplacés, sans sauvegarde.", sel.len())),
                    };
                    match (&only, &self.source) {
                        (Some(sel), _) if sel.len() == 1 => {
                            let rel = sel.iter().next().unwrap();
                            ui.label(RichText::new(rel.display().to_string()).strong());
                        }
                        (_, Some(src)) => {
                            ui.label(RichText::new(src.display().to_string()).strong());
                        }
                        _ => {}
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let confirm = egui::Button::new(RichText::new(theme::label(ICON_BOLT, "Confirmer")).color(Color32::WHITE)).fill(ERR);
                        if ui.add(confirm).clicked() {
                            decision = Some(true);
                        }
                        if ui.button("Annuler").clicked() {
                            decision = Some(false);
                        }
                    });
                });
            if let Some(go) = decision {
                self.confirm_in_place = None;
                if go {
                    self.start_optimize(ctx, only);
                }
            }
        }
        if let Some(err) = self.error.clone() {
            egui::Window::new(theme::label(ICON_ERROR, "Erreur"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(RichText::new(err).color(ERR));
                    ui.add_space(8.0);
                    if ui.button("OK").clicked() {
                        self.error = None;
                    }
                });
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_job(&ctx);

        // Closing mid-run would leave an export half-written: ask first.
        if ctx.input(|i| i.viewport().close_requested()) && self.busy() && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }

        let bar = |fill| egui::Frame::new().fill(fill).inner_margin(Margin::symmetric(10, 8));
        egui::Panel::top("top").frame(bar(BG)).show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").frame(bar(BG)).show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("side")
            .resizable(false)
            .exact_size(380.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 0)))
            .show(ui, |ui| self.side_panel(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::symmetric(16, 10)))
            .show(ui, |ui| {
                self.tab_bar(ui);
                match self.tab {
                    Tab::Analysis => self.analysis_tab(ui),
                    Tab::Comparison => self.comparison_tab(ui),
                    Tab::Report => self.report_tab(ui),
                }
            });
        self.dialogs(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let saved = Saved {
            method: self.methods[self.method].id().to_owned(),
            source: self.source.clone(),
            export_dir: self.export_dir.clone(),
            output: self.output,
            overwrite: self.overwrite,
            settings: self.settings.clone(),
        };
        eframe::set_value(storage, eframe::APP_KEY, &saved);
    }
}

/// Registered methods. Vehicles will be added here once implemented.
fn registered_methods() -> Vec<Arc<dyn Method>> {
    vec![
        Arc::new(optistream_clothing::ClothingMethod::new()),
        Arc::new(optistream_mapping::MappingMethod::new()),
    ]
}

/// One half of a before/after comparison.
fn side(ui: &mut Ui, title: &str, desc: &str, tex: Option<&TextureHandle>, size: Vec2, color: Color32) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong().color(color));
            ui.label(RichText::new(desc).small().color(WEAK));
        });
        match tex {
            Some(t) => {
                egui::Frame::new().fill(BG).corner_radius(CornerRadius::same(6)).show(ui, |ui| {
                    ui.add(egui::Image::new((t.id(), size)));
                });
            }
            None => {
                egui::Frame::new()
                    .fill(BG)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(6))
                    .show(ui, |ui| {
                        ui.set_min_size(size.max(Vec2::new(200.0, 80.0)));
                        ui.centered_and_justified(|ui| {
                            ui.label(RichText::new(theme::label(ICON_CHECK_CIRCLE, "Inchangé")).color(WEAK));
                        });
                    });
            }
        }
    });
}

fn load_image(ctx: &egui::Context, name: &str, img: &PreviewImage) -> TextureHandle {
    let color = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba);
    ctx.load_texture(name, color, egui::TextureOptions::LINEAR)
}

fn load_texture_preview(ctx: &egui::Context, name: &str, img: &PreviewImage, show_alpha: bool) -> TextureHandle {
    if show_alpha {
        return load_image(ctx, name, img);
    }
    let mut opaque = img.clone();
    for px in opaque.rgba.chunks_exact_mut(4) {
        px[3] = 255;
    }
    load_image(ctx, name, &opaque)
}

fn load_preview(ctx: &egui::Context, i: usize, item: PreviewItem, show_alpha: bool) -> LoadedPreview {
    let (before, after) = match &item.content {
        PreviewContent::Texture { before, after } => (
            Some(load_texture_preview(ctx, &format!("tb{i}"), before, show_alpha)),
            after.as_ref().map(|a| load_texture_preview(ctx, &format!("ta{i}"), a, show_alpha)),
        ),
        PreviewContent::Mesh { .. } => (None, None),
    };
    LoadedPreview { item, before, after, frame: None, rendered: None }
}
