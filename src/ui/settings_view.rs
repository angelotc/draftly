use egui::{Align, Color32, Layout, RichText, Ui};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::domain::config::{SettingsValues, load_config_strict, save_settings_form};
use crate::domain::hotkey::{self, Hotkey};
use crate::engine::controller::Controller;
use crate::tray::{TrayHandle, toggle_label};

use crate::ui::theme::{ACCENT, MUTED, NEG, POS, TEXT};
use crate::win32;

pub struct SettingsUiState {
    pub open: bool,
    pub path: PathBuf,
    pub picks_shown: usize,
    pub toggle: String,
    capturing: bool,
    tray: Arc<Mutex<Option<TrayHandle>>>,
    pub min_pick_rate: String,
    pub comfort_only: bool,
    pub threat_scaling: bool,
    pub comfort_weight: String,
    pub lockfile: String,
    pub errors: HashMap<&'static str, String>,
    pub message: Option<(String, bool)>,
    pub version_text: String,
}

impl SettingsUiState {
    pub fn new(path: PathBuf, version_text: String, tray: Arc<Mutex<Option<TrayHandle>>>) -> Self {
        let mut st = Self {
            open: false,
            path,
            picks_shown: 5,
            toggle: Hotkey::DEFAULT.to_string(),
            capturing: false,
            tray,
            min_pick_rate: "1.0".into(),
            comfort_only: false,
            threat_scaling: true,
            comfort_weight: "1.0".into(),
            lockfile: String::new(),
            errors: HashMap::new(),
            message: None,
            version_text,
        };
        st.reload_from_disk();
        st
    }

    pub fn reload_from_disk(&mut self) {
        let vals = load_config_strict(Some(&self.path))
            .map(|c| SettingsValues::from_config(&c))
            .unwrap_or_else(|_| SettingsValues::defaults());
        self.apply_values(&vals);
        self.errors.clear();
        self.message = None;
    }

    pub fn apply_values(&mut self, v: &SettingsValues) {
        self.picks_shown = v.picks_shown;
        self.toggle = Hotkey::parse(&v.toggle).map_or_else(|_| v.toggle.clone(), |hk| hk.to_string());
        self.capturing = false;
        self.min_pick_rate = format!("{}", v.min_pick_rate);
        self.comfort_only = v.comfort_only;
        self.threat_scaling = v.threat_scaling;
        self.comfort_weight = format!("{}", v.comfort_weight);
        self.lockfile = v.lockfile.clone();
    }
}

impl SettingsUiState {
    fn save(&mut self, ctl: &Arc<Controller>) {
        let vals = SettingsValues {
            picks_shown: self.picks_shown,
            toggle: self.toggle.clone(),
            min_pick_rate: self.min_pick_rate.trim().parse::<f64>().unwrap_or(f64::NAN),
            comfort_only: self.comfort_only,
            threat_scaling: self.threat_scaling,
            comfort_weight: self.comfort_weight.trim().parse::<f64>().unwrap_or(f64::NAN),
            lockfile: self.lockfile.clone(),
        };
        let errs = vals.validate();
        if !errs.is_empty() {
            self.errors = errs;
            self.message = None;
            return;
        }
        let Ok(hk) = Hotkey::parse(&vals.toggle) else { return };
        match save_settings_form(&vals, &self.path) {
            Ok(errs) if errs.is_empty() => {
                self.errors.clear();
                self.message = Some(("Saved.".into(), true));
                if hk != hotkey::current() {
                    hotkey::set_current(hk);
                    if let Some(t) = self.tray.lock().unwrap().as_ref() {
                        t.set_toggle_label(toggle_label());
                    }
                }
                if let Ok(cfg) = load_config_strict(Some(&self.path)) {
                    let ctl_c = Arc::clone(ctl);
                    tokio::spawn(async move {
                        ctl_c.reload_config(cfg).await;
                    });
                }
            }
            Ok(errs) => {
                self.errors = errs;
                self.message = None;
            }
            Err(e) => self.message = Some((e, false)),
        }
    }

    fn capture_hotkey(&mut self, ui: &Ui) {
        if !self.capturing {
            return;
        }
        let events = ui.input(|i| i.events.clone());
        for ev in events {
            let egui::Event::Key { key, pressed: true, modifiers, repeat: false, .. } = ev else { continue };
            if key == egui::Key::Escape {
                self.capturing = false;
                self.errors.remove("toggle");
                return;
            }
            let mut text = String::new();
            for (on, name) in [(modifiers.ctrl, "ctrl+"), (modifiers.shift, "shift+"), (modifiers.alt, "alt+")] {
                if on {
                    text.push_str(name);
                }
            }
            text.push_str(key.name());
            match Hotkey::parse(&text) {
                Ok(hk) => {
                    self.toggle = hk.to_string();
                    self.errors.remove("toggle");
                    self.capturing = false;
                }
                Err(e) => {
                    self.errors.insert("toggle", e);
                }
            }
            return;
        }
    }
}

fn hotkey_field(ui: &mut Ui, st: &mut SettingsUiState) {
    ui.label(RichText::new("Show or hide Draftly").size(12.0).color(TEXT));
    let text = if st.capturing { "Press keys… (Esc to cancel)".to_string() } else { st.toggle.clone() };
    let btn = egui::Button::new(RichText::new(text).size(12.0).color(if st.capturing { ACCENT } else { TEXT }))
        .min_size(egui::vec2(180.0, 0.0));
    if ui.add(btn).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        st.capturing = true;
        st.errors.remove("toggle");
    }
    if let Some(err) = st.errors.get("toggle") {
        ui.label(RichText::new(err).size(10.5).color(NEG));
    }
    ui.add_space(8.0);
}

pub fn render_settings(ui: &mut Ui, st: &mut SettingsUiState, ctl: &Arc<Controller>) {
    ui.label(RichText::new("Settings").size(16.0).strong().color(ACCENT));
    ui.label(RichText::new(&st.version_text).size(10.5).color(MUTED));
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    st.capture_hotkey(ui);

    ui.label(RichText::new("Picks shown").size(12.0).color(TEXT));
    ui.add(egui::Slider::new(&mut st.picks_shown, 1..=15));
    if let Some(err) = st.errors.get("picks_shown") {
        ui.label(RichText::new(err).size(10.5).color(NEG));
    }
    ui.add_space(8.0);

    ui.label(RichText::new("Minimum pick rate (%)").size(12.0).color(TEXT));
    ui.text_edit_singleline(&mut st.min_pick_rate);
    if let Some(err) = st.errors.get("min_pick_rate") {
        ui.label(RichText::new(err).size(10.5).color(NEG));
    }
    ui.add_space(8.0);

    ui.checkbox(&mut st.comfort_only, "Only recommend champions you've played or have mastery on")
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.add_space(8.0);

    ui.checkbox(&mut st.threat_scaling, "Adapt build to fed enemies")
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.add_space(8.0);

    ui.label(RichText::new("Record and mastery weight (0 = off, up to 2)").size(12.0).color(TEXT));
    ui.text_edit_singleline(&mut st.comfort_weight);
    if let Some(err) = st.errors.get("comfort_weight") {
        ui.label(RichText::new(err).size(10.5).color(NEG));
    }
    ui.add_space(8.0);

    ui.label(RichText::new("League lockfile path").size(12.0).color(TEXT));
    ui.text_edit_singleline(&mut st.lockfile);
    if let Some(err) = st.errors.get("lockfile") {
        ui.label(RichText::new(err).size(10.5).color(NEG));
    }
    ui.add_space(8.0);

    hotkey_field(ui, st);
    ui.add_space(4.0);

    if let Some((msg, ok)) = &st.message {
        ui.label(RichText::new(msg).size(11.0).color(if *ok { POS } else { NEG }));
        ui.add_space(6.0);
    }

    ui.horizontal(|ui| {
        if ui.button("Save").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            st.save(ctl);
        }
        if ui.button("Reset defaults").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            st.apply_values(&SettingsValues::defaults());
            st.errors.clear();
        }
        if ui.button("Open config.toml").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            win32::open_file(&st.path);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Close").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                st.open = false;
            }
        });
    });
    let _ = Color32::TRANSPARENT;
}
