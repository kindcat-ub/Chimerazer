//! `chimerazer-panel` — minimal functional panel (M3).
//!
//! Shows the device state and offers only the writes that are verified for
//! the connected link ([`Param::verified_for`] on the exact value). All
//! protocol work happens in `chimerazer-devices`; this file only lays out
//! controls and results.

mod worker;

use std::collections::BTreeMap;

use chimerazer_devices::naga_pro::params::Param;
use chimerazer_devices::naga_pro::restore::{Entry, RestoreReport};
use chimerazer_devices::naga_pro::{self, LED_LOGO, LED_SCROLL_WHEEL, LIVE_STORAGE, state};
use chimerazer_devices::{Trace, is_control_interface, lookup};
use chimerazer_transport::enumerate;
use eframe::egui;

use worker::{Device, Done, Job, Worker};

fn main() -> eframe::Result {
    eframe::run_native(
        "Chimerazer",
        eframe::NativeOptions::default(),
        Box::new(|cc| Ok(Box::new(Panel::new(&cc.egui_ctx)))),
    )
}

/// Parameters the panel can edit, profile 1 first.
fn editable() -> Vec<Param> {
    let mut out = Vec::new();
    for storage in [1, LIVE_STORAGE] {
        out.push(Param::Dpi { storage });
        out.push(Param::DpiStages { storage });
        for led in [LED_LOGO, LED_SCROLL_WHEEL] {
            out.push(Param::Brightness { storage, led });
            out.push(Param::Effect { storage, led });
        }
    }
    out
}

fn hint(param: Param) -> &'static str {
    match param {
        Param::PollRate => "1000 | 500 | 125",
        Param::Dpi { .. } => "800 or 800x400",
        Param::DpiStages { .. } => "active=<stage id>",
        Param::Brightness { .. } => "0..255",
        Param::Effect { .. } => "RRGGBB or spectrum",
    }
}

/// Read-only values shown in the summary.
const SUMMARY: &[(&str, &str)] = &[
    ("firmware", "device.firmware"),
    ("active profile", "profiles.active"),
    ("poll rate (read-only)", "device.poll_rate"),
    ("battery", "volatile.battery"),
    ("charging", "volatile.charging"),
];

struct Panel {
    worker: Worker,
    busy: bool,
    devices: Vec<Device>,
    selected: Option<usize>,
    snapshot: Option<BTreeMap<String, String>>,
    inputs: BTreeMap<String, String>,
    filter: String,
    dump_path: String,
    dump: Option<BTreeMap<String, String>>,
    preview: Option<RestoreReport>,
    log: Vec<(String, Vec<Trace>)>,
}

impl Panel {
    fn new(ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        let mut panel = Self {
            worker: Worker::spawn(move || ctx.request_repaint()),
            busy: false,
            devices: Vec::new(),
            selected: None,
            snapshot: None,
            inputs: BTreeMap::new(),
            filter: String::new(),
            dump_path: "dump.txt".into(),
            dump: None,
            preview: None,
            log: Vec::new(),
        };
        panel.refresh_devices();
        panel
    }

    fn device(&self) -> Option<&Device> {
        self.selected.and_then(|i| self.devices.get(i))
    }

    fn note(&mut self, line: impl Into<String>) {
        self.log.push((line.into(), Vec::new()));
    }

    fn refresh_devices(&mut self) {
        match enumerate() {
            Ok(all) => {
                self.devices = all
                    .into_iter()
                    .filter(|h| is_control_interface(h.vendor_id, h.feature_report_len))
                    .filter_map(|h| {
                        let (model, usb) = lookup(h.product_id)?;
                        (*model == naga_pro::MODEL).then_some(Device { node: h.node, usb })
                    })
                    .collect();
                self.selected = (!self.devices.is_empty()).then_some(0);
                self.snapshot = None;
                self.preview = None;
            }
            Err(e) => self.note(format!("cannot read sysfs: {e}")),
        }
    }

    fn submit(&mut self, job: Job) {
        if let Some(device) = self.device().cloned() {
            self.busy = true;
            self.worker.submit(device, job);
        }
    }

    fn receive(&mut self) {
        while let Ok(done) = self.worker.done.try_recv() {
            self.busy = false;
            match done {
                Done::Snapshot(Ok(s)) => {
                    let errors = s
                        .values
                        .values()
                        .filter(|v| v.starts_with("error:"))
                        .count();
                    self.note(format!("read {} values, {errors} errors", s.values.len()));
                    self.snapshot = Some(s.values);
                }
                Done::Set(Ok((entries, s))) => {
                    for e in entries {
                        self.log_entry("set", e);
                    }
                    self.snapshot = Some(s.values);
                }
                Done::Restore(Ok(report), dry_run) => {
                    self.log_restore(&report, dry_run);
                    if !report.after.is_empty() {
                        self.snapshot = Some(report.after.clone());
                    }
                    self.preview = dry_run.then_some(report);
                }
                Done::Snapshot(Err(e)) | Done::Set(Err(e)) | Done::Restore(Err(e), _) => {
                    self.note(format!("error: {e}"))
                }
            }
        }
    }

    fn log_entry(&mut self, what: &str, e: Entry) {
        let line = format!(
            "{what} {}: {} -> {}: {}",
            e.param,
            e.before.as_deref().unwrap_or("-"),
            e.after.as_deref().unwrap_or("-"),
            e.outcome
        );
        self.log.push((line, e.traces));
    }

    fn log_restore(&mut self, r: &RestoreReport, dry_run: bool) {
        let kind = if dry_run {
            "restore (dry run)"
        } else {
            "restore"
        };
        if let Some(why) = &r.refused {
            self.note(format!("{kind} refused, nothing written: {why}"));
            return;
        }
        for e in &r.entries {
            self.log
                .push((format!("{kind} {}: {}", e.key, e.outcome), e.traces.clone()));
        }
        let verdict = if dry_run {
            "nothing written"
        } else if r.ok() {
            "OK"
        } else {
            "FAILED"
        };
        self.note(format!(
            "{kind}: {verdict}; restorable values still differing {}, other differences {}, keys not restorable {}",
            r.unrestored().len(),
            r.outside_scope().len(),
            r.not_restorable.len()
        ));
    }

    fn device_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Find devices").clicked() {
                self.refresh_devices();
            }
            let label = |d: &Device| {
                format!(
                    "{:04x} {:?} {}",
                    d.usb.product_id,
                    d.usb.link,
                    d.node.display()
                )
            };
            let current = self.device().map_or("no Naga Pro found".to_string(), label);
            egui::ComboBox::from_id_salt("device")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for i in 0..self.devices.len() {
                        let text = label(&self.devices[i]);
                        if ui
                            .selectable_value(&mut self.selected, Some(i), text)
                            .clicked()
                        {
                            self.snapshot = None;
                            self.preview = None;
                        }
                    }
                });
            let can = self.device().is_some() && !self.busy;
            if ui
                .add_enabled(can, egui::Button::new("Read state"))
                .clicked()
            {
                self.submit(Job::Snapshot);
            }
            if self.busy {
                ui.spinner();
            }
        });
    }

    fn summary(&self, ui: &mut egui::Ui, snap: &BTreeMap<String, String>) {
        egui::Grid::new("summary").show(ui, |ui| {
            for (label, key) in SUMMARY {
                ui.label(*label);
                ui.label(snap.get(*key).map_or("-", String::as_str));
                ui.end_row();
            }
        });
    }

    fn settings(&mut self, ui: &mut egui::Ui, snap: &BTreeMap<String, String>) {
        let Some(link) = self.device().map(|d| d.usb.link) else {
            return;
        };
        let mut job = None;
        egui::Grid::new("settings").striped(true).show(ui, |ui| {
            for param in editable() {
                let key = param.dump_key();
                let current = snap.get(&key).map(|t| param.from_dump(t));
                ui.label(param.to_string());
                ui.label(match &current {
                    Some(Ok(v)) => param.render(v),
                    Some(Err(e)) => format!("unreadable: {e}"),
                    None => "-".into(),
                });
                let input = self.inputs.entry(key.clone()).or_default();
                ui.add(
                    egui::TextEdit::singleline(input)
                        .hint_text(hint(param))
                        .desired_width(140.0),
                );
                if let Param::Effect { .. } = param {
                    let mut rgb = u32::from_str_radix(input.trim_start_matches('#'), 16)
                        .map(|v| {
                            let [_, r, g, b] = v.to_be_bytes();
                            [r, g, b]
                        })
                        .unwrap_or([0, 0, 0]);
                    if egui::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
                        *input = format!("{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                    }
                } else {
                    ui.label("");
                }
                // Enabled only if this exact value is verified on this link.
                let check = match &current {
                    Some(Ok(cur)) if !input.is_empty() => param.value(input, cur).and_then(|v| {
                        if &v == cur {
                            Err("already set".into())
                        } else if param.verified_for(link, &v) {
                            Ok(())
                        } else {
                            Err(format!(
                                "writing {} is not verified for {link:?}",
                                param.render(&v)
                            ))
                        }
                    }),
                    Some(Ok(_)) => Err("enter a value".into()),
                    _ => Err("read the state first".into()),
                };
                let write = ui.add_enabled(check.is_ok() && !self.busy, egui::Button::new("Write"));
                if write.clicked() {
                    job = Some(Job::Set(vec![(param, input.clone())]));
                }
                if let Err(why) = check {
                    write.on_disabled_hover_text(why);
                }
                ui.end_row();
            }
        });
        if let Some(job) = job {
            self.submit(job);
        }
    }

    fn dump_block(&mut self, ui: &mut egui::Ui, snap: Option<&BTreeMap<String, String>>) {
        ui.horizontal(|ui| {
            ui.label("file");
            ui.text_edit_singleline(&mut self.dump_path);
            if ui
                .add_enabled(snap.is_some(), egui::Button::new("Save dump"))
                .clicked()
                && let (Some(snap), Some(d)) = (snap, self.device())
            {
                let text = format!(
                    "# chimerazer dump\n# model {}\n# device 1532:{:04x} {:?}\n{}",
                    naga_pro::MODEL.name,
                    d.usb.product_id,
                    d.usb.link,
                    state::Snapshot {
                        values: snap.clone(),
                        traces: Vec::new()
                    }
                    .to_text()
                );
                let msg = match std::fs::write(&self.dump_path, text) {
                    Ok(()) => format!("saved {}", self.dump_path),
                    Err(e) => format!("{}: {e}", self.dump_path),
                };
                self.note(msg);
            }
            if ui.button("Load dump").clicked() {
                self.preview = None;
                match std::fs::read_to_string(&self.dump_path) {
                    Ok(text)
                        if text
                            .lines()
                            .any(|l| l == format!("# model {}", naga_pro::MODEL.name)) =>
                    {
                        self.dump = Some(state::parse_text(&text));
                        let msg = format!("loaded {}", self.dump_path);
                        self.note(msg);
                    }
                    Ok(_) => {
                        let msg =
                            format!("{} is not a {} dump", self.dump_path, naga_pro::MODEL.name);
                        self.note(msg);
                    }
                    Err(e) => {
                        let msg = format!("{}: {e}", self.dump_path);
                        self.note(msg);
                    }
                }
            }
        });
        let Some(dump) = self.dump.clone() else {
            return;
        };
        if let Some(snap) = snap {
            let diffs = state::compare(&dump, snap);
            egui::CollapsingHeader::new(format!("dump vs device: {} differences", diffs.len()))
                .id_salt("diff")
                .show(ui, |ui| {
                    for (k, a, b) in diffs {
                        ui.label(format!(
                            "{k}: dump {} / device {}",
                            a.unwrap_or_default(),
                            b.unwrap_or_default()
                        ));
                    }
                });
        }
        ui.horizontal(|ui| {
            let idle = self.device().is_some() && !self.busy;
            if ui
                .add_enabled(idle, egui::Button::new("Preview restore (dry run)"))
                .clicked()
            {
                self.submit(Job::Restore {
                    dump: dump.clone(),
                    dry_run: true,
                });
            }
            let ready = self.preview.as_ref().is_some_and(|p| p.refused.is_none());
            let apply = ui.add_enabled(idle && ready, egui::Button::new("Apply restore"));
            if apply.clicked() {
                self.preview = None;
                self.submit(Job::Restore {
                    dump,
                    dry_run: false,
                });
            }
            apply.on_disabled_hover_text(
                "run the preview first; values are written one at a time, without atomicity",
            );
        });
        if let Some(p) = &self.preview {
            egui::Grid::new("preview").striped(true).show(ui, |ui| {
                for e in &p.entries {
                    ui.label(&e.key);
                    ui.label(e.target.as_deref().unwrap_or("-"));
                    ui.label(e.outcome.to_string());
                    ui.end_row();
                }
            });
        }
    }

    fn state_table(&mut self, ui: &mut egui::Ui, snap: &BTreeMap<String, String>) {
        ui.horizontal(|ui| {
            ui.label("filter");
            ui.text_edit_singleline(&mut self.filter);
        });
        egui::Grid::new("state").striped(true).show(ui, |ui| {
            for (k, v) in snap
                .iter()
                .filter(|(k, _)| k.contains(self.filter.as_str()))
            {
                ui.label(k);
                ui.label(v);
                ui.end_row();
            }
        });
    }

    fn log_view(&self, ui: &mut egui::Ui) {
        for (i, (line, traces)) in self.log.iter().enumerate().rev() {
            if traces.is_empty() {
                ui.label(line);
            } else {
                egui::CollapsingHeader::new(line)
                    .id_salt(("log", i))
                    .show(ui, |ui| {
                        for t in traces {
                            ui.monospace(format!("> {}", hex(&t.request)));
                            for r in &t.responses {
                                ui.monospace(format!("< {}", hex(r)));
                            }
                        }
                    });
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

impl eframe::App for Panel {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive();
        egui::TopBottomPanel::top("device").show(ctx, |ui| self.device_bar(ui));
        egui::TopBottomPanel::bottom("log")
            .resizable(true)
            .default_height(160.0)
            .show(ctx, |ui| {
                ui.heading("Log");
                egui::ScrollArea::vertical().show(ui, |ui| self.log_view(ui));
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                let snap = self.snapshot.clone();
                if let Some(snap) = &snap {
                    ui.heading("State");
                    self.summary(ui, snap);
                    ui.separator();
                    ui.heading("Settings");
                    ui.label(
                        "Only writes verified for this connection are enabled; each is read back.",
                    );
                    self.settings(ui, snap);
                    ui.separator();
                } else {
                    ui.label("Read the state to see values and settings.");
                }
                ui.heading("Dump and restore");
                self.dump_block(ui, snap.as_ref());
                if let Some(snap) = &snap {
                    ui.separator();
                    egui::CollapsingHeader::new(format!("All values ({})", snap.len()))
                        .id_salt("all")
                        .show(ui, |ui| self.state_table(ui, snap));
                }
            });
        });
    }
}
