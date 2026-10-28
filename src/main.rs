//! open-rode: native Linux GUI for the RØDE Wireless GO II.
//!
//! Device I/O runs on a worker thread; the UI sends `Action`s and receives
//! `Snapshot`s so a slow HID transaction never freezes the window.

use eframe::egui::{self, Color32, RichText};
use hidapi::HidApi;
use open_rode::audio::{dbfs, AudioSource, Meter};
use open_rode::device::{device_now, GoII, Snapshot};
use open_rode::protocol::{self as p, ChannelMode, GainMode, RecordMode, RxButton, TxButton, TxLed};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

enum Action {
    Gain(GainMode, u8),
    Channel(ChannelMode, bool),
    Safety(bool),
    AutoDim(bool),
    RxButton(RxButton),
    SyncClock,
    TxLed(TxLed),
    TxPad(bool),
    TxCompress(bool),
    TxRecord(RecordMode, TxButton),
}

enum Event {
    Connected(String),
    Disconnected(String),
    Snapshot(Snapshot),
    Error(String),
}

fn worker(rx: Receiver<Action>, tx: Sender<Event>) {
    let api = match HidApi::new() {
        Ok(a) => a,
        Err(e) => { let _ = tx.send(Event::Error(format!("hidapi: {e}"))); return; }
    };
    let mut dev: Option<GoII> = None;
    let mut last_poll = Instant::now() - Duration::from_secs(10);
    loop {
        if dev.is_none() {
            match GoII::find(&api).ok().and_then(|v| v.into_iter().next()) {
                Some(info) => match GoII::open(&api, &info) {
                    Ok(d) => { let _ = tx.send(Event::Connected(info.serial.clone())); dev = Some(d); }
                    Err(e) => { let _ = tx.send(Event::Error(format!("{e:#}"))); thread::sleep(Duration::from_secs(2)); }
                },
                None => { thread::sleep(Duration::from_millis(800)); continue; }
            }
        }
        let d = dev.as_ref().unwrap();
        let mut result = Ok(());
        while let Ok(a) = rx.try_recv() {
            result = result.and(apply(d, a));
            last_poll = Instant::now() - Duration::from_secs(10);
        }
        if result.is_ok() && last_poll.elapsed() > Duration::from_millis(1500) {
            result = d.snapshot().map(|s| { let _ = tx.send(Event::Snapshot(s)); });
            last_poll = Instant::now();
        }
        if let Err(e) = result {
            let _ = tx.send(Event::Disconnected(format!("{e:#}")));
            dev = None;
            continue;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn apply(d: &GoII, a: Action) -> anyhow::Result<()> {
    match a {
        Action::Gain(m, v) => d.set_gain(m, v),
        Action::Channel(m, s) => d.set_channel_mode(m, s),
        Action::Safety(s) => d.set_safety(s),
        Action::AutoDim(s) => d.set_auto_dim(s),
        Action::RxButton(b) => d.set_rx_button(b),
        Action::SyncClock => {
            let t = device_now();
            d.set_rx_clock(t)?;
            d.set_tx_clock(t)
        }
        Action::TxLed(l) => d.set_tx_led(l),
        Action::TxPad(s) => d.set_tx_pad(s),
        Action::TxCompress(s) => d.set_tx_compress(s),
        Action::TxRecord(r, b) => d.set_tx_record_and_button(r, b),
    }
}

/// UI state. Most fields are refreshed from each `Snapshot`; `coarse_level`,
/// `tx_pad` and the Manual/Record distinction are not reported by the device
/// and are mirrored from what the user last set.
struct App {
    to_worker: Sender<Action>,
    from_worker: Receiver<Event>,
    snap: Option<Snapshot>,
    status: String,
    connected: bool,
    channel: ChannelMode,
    safety: bool,
    auto_dim: bool,
    rx_button: RxButton,
    gain_mode: GainMode,
    fine_step: u8,
    coarse_level: u8,
    rec_mode: RecordMode,
    tx_button: TxButton,
    tx_pad: bool,
    audio_src: AudioSource,
    meter: Option<Meter>,
    meter_err: String,
    meter_next_try: Instant,
    /// smoothed RMS and held peak per channel, in dBFS
    meter_rms: [f32; 2],
    meter_peak: [f32; 2],
    meter_peak_at: [Instant; 2],
}

impl App {
    fn new() -> Self {
        let (to_worker, rx) = mpsc::channel();
        let (tx, from_worker) = mpsc::channel();
        thread::spawn(move || worker(rx, tx));
        App {
            to_worker, from_worker,
            snap: None, status: "Looking for a Wireless GO II receiver…".into(), connected: false,
            channel: ChannelMode::Split, safety: false, auto_dim: true, rx_button: RxButton::AutoDim,
            gain_mode: GainMode::Fine, fine_step: 10, coarse_level: 0,
            rec_mode: RecordMode::Off, tx_button: TxButton::Marker, tx_pad: false,
            audio_src: AudioSource::default(), meter: None, meter_err: String::new(),
            meter_next_try: Instant::now(), meter_rms: [-140.0; 2], meter_peak: [-140.0; 2],
            meter_peak_at: [Instant::now(); 2],
        }
    }

    /// Keep the capture stream alive while a receiver is connected; retry
    /// quietly if the sound card is not up yet or is busy.
    fn tick_audio(&mut self) {
        if !self.connected {
            self.meter = None;
            return;
        }
        if self.meter.is_none() && Instant::now() >= self.meter_next_try {
            self.meter_next_try = Instant::now() + Duration::from_secs(3);
            self.audio_src = AudioSource::discover();
            if self.audio_src.present() {
                match Meter::open(&self.audio_src) {
                    Ok(m) => { self.meter = Some(m); self.meter_err.clear(); }
                    Err(e) => self.meter_err = format!("{e:#}"),
                }
            } else {
                self.meter_err = "sound card not found (snd-usb-audio)".into();
            }
        }
        if let Some(m) = &self.meter {
            let lv = m.read();
            for c in 0..2 {
                let rms = dbfs(lv[c].rms);
                let peak = dbfs(lv[c].peak);
                // fast attack, ~20 dB/s release
                self.meter_rms[c] = if rms > self.meter_rms[c] { rms } else { (self.meter_rms[c] - 0.7).max(rms) };
                if peak >= self.meter_peak[c] || self.meter_peak_at[c].elapsed() > Duration::from_millis(1500) {
                    self.meter_peak[c] = peak;
                    self.meter_peak_at[c] = Instant::now();
                }
            }
        }
    }

    fn send(&self, a: Action) {
        let _ = self.to_worker.send(a);
    }

    fn drain(&mut self) {
        while let Ok(ev) = self.from_worker.try_recv() {
            match ev {
                Event::Connected(s) => { self.connected = true; self.status = format!("Connected (serial {s})"); }
                Event::Disconnected(e) => { self.connected = false; self.snap = None; self.status = format!("Disconnected: {e}"); }
                Event::Error(e) => self.status = e,
                Event::Snapshot(s) => {
                    self.channel = if s.merged { ChannelMode::Merged } else { ChannelMode::Split };
                    self.safety = s.safety;
                    self.auto_dim = s.auto_dim;
                    self.rx_button = s.rx_button;
                    if let Some(m) = s.gain_mode { self.gain_mode = m; }
                    match s.gain_mode {
                        Some(GainMode::Fine) => self.fine_step = s.gain_value.min(10),
                        Some(GainMode::Coarse) => self.coarse_level = s.gain_value.min(2),
                        _ => {}
                    }
                    if let Some(t) = &s.tx {
                        self.rec_mode = match (t.record_mode(), self.rec_mode) {
                            (RecordMode::Off, RecordMode::Manual) => RecordMode::Manual,
                            (m, _) => m,
                        };
                        self.tx_button = match (t.button(), self.tx_button) {
                            (TxButton::Disabled, TxButton::Record) => TxButton::Record,
                            (b, _) => b,
                        };
                    }
                    self.snap = Some(s);
                }
            }
        }
    }
}

fn battery_text(pct: u8) -> RichText {
    let c = if pct <= 20 { Color32::from_rgb(220, 70, 70) } else if pct <= 50 { Color32::from_rgb(230, 180, 60) } else { Color32::from_rgb(90, 200, 110) };
    RichText::new(format!("{pct}%")).color(c).strong()
}

/// Horizontal level bar, -70..0 dBFS, with a held peak tick.
fn meter_bar(ui: &mut egui::Ui, label: &str, rms_db: f32, peak_db: f32) {
    ui.horizontal(|ui| {
        ui.add_sized([34.0, 18.0], egui::Label::new(label));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width() - 70.0, 14.0), egui::Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 3.0, Color32::from_gray(40));
        let frac = |db: f32| ((db + 70.0) / 70.0).clamp(0.0, 1.0);
        let fill = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * frac(rms_db), rect.height()));
        let color = if rms_db > -6.0 { Color32::from_rgb(220, 70, 70) } else if rms_db > -18.0 { Color32::from_rgb(230, 180, 60) } else { Color32::from_rgb(90, 200, 110) };
        p.rect_filled(fill, 3.0, color);
        for mark in [-60.0, -48.0, -36.0, -24.0, -12.0] {
            let x = rect.min.x + rect.width() * frac(mark);
            p.line_segment([egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)], (1.0, Color32::from_gray(70)));
        }
        if peak_db > -70.0 {
            let x = rect.min.x + rect.width() * frac(peak_db);
            p.line_segment([egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)], (2.0, Color32::WHITE));
        }
        let txt = if peak_db <= -100.0 { "silent".to_string() } else { format!("{peak_db:5.1} dB") };
        ui.add_sized([64.0, 18.0], egui::Label::new(RichText::new(txt).monospace().small()));
    });
}

fn seg<T: PartialEq + Copy>(ui: &mut egui::Ui, current: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        for (v, label) in options {
            if ui.selectable_label(*current == *v, *label).clicked() && *current != *v {
                *current = *v;
                changed = true;
            }
        }
    });
    changed
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain();
        self.tick_audio();
        ctx.request_repaint_after(if self.meter.is_some() { Duration::from_millis(33) } else { Duration::from_millis(500) });

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Open Rode");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(&self.status);
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(snap) = self.snap.clone() else {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label("Plug in the Wireless GO II receiver over USB.");
                    ui.small("If it is plugged in, check /etc/udev/rules.d/70-rode.rules.");
                });
                return;
            };

            ui.columns(2, |cols| {
                // ---------------- RX ----------------
                let ui = &mut cols[0];
                ui.group(|ui| {
                    ui.heading(format!("Wireless GO II RX  ·  {}", snap.name));
                    ui.horizontal(|ui| {
                        ui.label("Battery"); ui.label(battery_text(snap.rx_battery));
                        ui.separator();
                        ui.label(format!("Serial {:08X}", snap.serial));
                    });
                    ui.add_space(8.0);

                    ui.strong("Channel routing");
                    let mut ch = self.channel;
                    if seg(ui, &mut ch, &[(ChannelMode::Split, "Split"), (ChannelMode::Merged, "Merged")]) {
                        self.channel = ch;
                        self.send(Action::Channel(ch, self.safety));
                    }
                    if ui.checkbox(&mut self.safety, "Safety channel (−20 dB on the right)").changed() {
                        self.send(Action::Safety(self.safety));
                    }
                    ui.add_space(8.0);

                    ui.strong("Output gain");
                    let mut gm = self.gain_mode;
                    if seg(ui, &mut gm, &[(GainMode::Coarse, "Coarse"), (GainMode::Fine, "Fine"), (GainMode::Camera, "Camera")]) {
                        self.gain_mode = gm;
                        let v = match gm { GainMode::Fine => self.fine_step, GainMode::Coarse => self.coarse_level, GainMode::Camera => 0 };
                        self.send(Action::Gain(gm, v));
                    }
                    match self.gain_mode {
                        GainMode::Fine => {
                            let mut step = self.fine_step as i32;
                            let r = ui.add(egui::Slider::new(&mut step, 0..=10).custom_formatter(|v, _| format!("{} dB", p::fine_step_db(v as u8))));
                            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                                self.fine_step = step as u8;
                                self.send(Action::Gain(GainMode::Fine, self.fine_step));
                            }
                        }
                        GainMode::Coarse => {
                            let mut lvl = self.coarse_level;
                            if seg(ui, &mut lvl, &[(0, "Low"), (1, "Medium"), (2, "High")]) {
                                self.coarse_level = lvl;
                                self.send(Action::Gain(GainMode::Coarse, lvl));
                            }
                        }
                        GainMode::Camera => { ui.small("Camera preset"); }
                    }
                    ui.add_space(8.0);

                    ui.strong("Ø button");
                    let mut b = self.rx_button;
                    if seg(ui, &mut b, &[(RxButton::AutoDim, "Toggle auto-dim"), (RxButton::Marker, "Drop marker")]) {
                        self.rx_button = b;
                        self.send(Action::RxButton(b));
                    }
                    if ui.checkbox(&mut self.auto_dim, "Auto-dim screen after 10 s").changed() {
                        self.send(Action::AutoDim(self.auto_dim));
                    }
                    ui.add_space(8.0);

                    ui.strong("Audio input");
                    match self.audio_src.pw_description.clone().or_else(|| self.audio_src.alsa_card.as_ref().map(|c| format!("hw:{c}"))) {
                        Some(name) => {
                            ui.horizontal_wrapped(|ui| {
                                ui.label("Shows up in apps as");
                                ui.label(RichText::new(name).strong());
                            });
                            ui.small(format!(
                                "2 ch, 24-bit, 48 kHz.  ALSA hw:{}  {}",
                                self.audio_src.alsa_card.clone().unwrap_or_default(),
                                self.audio_src.pw_node.clone().map(|n| format!("  PipeWire {n}")).unwrap_or_default()
                            ));
                        }
                        None => { ui.small(if self.meter_err.is_empty() { "looking for the sound card".to_string() } else { self.meter_err.clone() }); }
                    }
                    let left = if self.channel == ChannelMode::Merged { "Mix" } else { "TX1" };
                    let right = if self.safety { "Safe" } else if self.channel == ChannelMode::Merged { "Mix" } else { "TX2" };
                    meter_bar(ui, left, self.meter_rms[0], self.meter_peak[0]);
                    meter_bar(ui, right, self.meter_rms[1], self.meter_peak[1]);
                    if let Some(m) = &self.meter { ui.small(format!("via {}", m.via)); } else if !self.meter_err.is_empty() { ui.small(&self.meter_err); }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let drift = snap.rx_clock as i64 - device_now() as i64;
                        ui.label(format!("Device clock: {drift:+} s vs this PC"));
                        if ui.button("Sync clocks").clicked() { self.send(Action::SyncClock); }
                    });
                });

                // ---------------- TX ----------------
                let ui = &mut cols[1];
                ui.group(|ui| {
                    match &snap.tx {
                        None => { ui.heading("Wireless GO II TX"); ui.label("No transmitter paired / in range."); }
                        Some(t) => {
                            ui.heading("Wireless GO II TX 1");
                            ui.horizontal(|ui| {
                                ui.label("Battery"); ui.label(battery_text(t.battery));
                                ui.separator();
                                ui.label(format!("FW {}", t.firmware()));
                                ui.separator();
                                ui.label(format!("Serial {:08X}", t.serial));
                            });
                            ui.add_space(8.0);

                            ui.strong("On-board recording");
                            let mut rm = self.rec_mode;
                            if seg(ui, &mut rm, &[(RecordMode::Off, "Off"), (RecordMode::Always, "Always"), (RecordMode::Manual, "Manual"), (RecordMode::Backup, "Backup")]) {
                                self.rec_mode = rm;
                                if rm == RecordMode::Manual { self.tx_button = TxButton::Record; }
                                self.send(Action::TxRecord(rm, self.tx_button));
                            }
                            let mut compress = !t.uncompressed;
                            if ui.checkbox(&mut compress, "Compress recordings").changed() {
                                self.send(Action::TxCompress(compress));
                            }
                            ui.small("Changing these stops any recording in progress.");
                            ui.add_space(8.0);

                            ui.strong("Ø button");
                            let mut tb = self.tx_button;
                            if seg(ui, &mut tb, &[(TxButton::Disabled, "Disabled"), (TxButton::Mute, "Mute"), (TxButton::Marker, "Marker"), (TxButton::Record, "Record")]) {
                                self.tx_button = tb;
                                if tb == TxButton::Record { self.rec_mode = RecordMode::Manual; }
                                else if self.rec_mode == RecordMode::Manual { self.rec_mode = RecordMode::Off; }
                                self.send(Action::TxRecord(self.rec_mode, tb));
                            }
                            ui.add_space(8.0);

                            ui.strong("LEDs");
                            let mut led = if t.led == TxLed::Dim as u8 { TxLed::Dim } else { TxLed::Bright };
                            if seg(ui, &mut led, &[(TxLed::Bright, "Bright"), (TxLed::Dim, "Dim")]) {
                                self.send(Action::TxLed(led));
                            }
                            if ui.checkbox(&mut self.tx_pad, "Input PAD").changed() {
                                self.send(Action::TxPad(self.tx_pad));
                            }
                            ui.add_space(8.0);
                            ui.collapsing("Raw status", |ui| {
                                ui.monospace(hex::encode(t.raw));
                            });
                        }
                    }
                });
            });
        });
    }
}

fn main() -> eframe::Result {
    env_logger::init();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([980.0, 600.0]).with_title("Open Rode"),
        ..Default::default()
    };
    eframe::run_native("open-rode", opts, Box::new(|_| Ok(Box::new(App::new()))))
}
