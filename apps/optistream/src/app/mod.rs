//! Main window: resource type, source folder, settings, output, analysis, comparison, report.

mod bars;
mod dialogs;
mod preview;
mod side_panel;
mod state;
mod tabs;
mod tasks;

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, Margin, Ui};
use optistream_core::{Analysis, Method, Report, Settings};
use serde::{Deserialize, Serialize};

use crate::app::preview::LoadedPreview;
use crate::app::state::Remaining;
use crate::jobs::Job;
use crate::theme::{self, BG};

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

/// Registered methods.
fn registered_methods() -> Vec<Arc<dyn Method>> {
    vec![
        Arc::new(optistream_clothing::ClothingMethod::new()),
        Arc::new(optistream_mapping::MappingMethod::new()),
        Arc::new(optistream_vehicle::VehicleMethod::new()),
    ]
}
