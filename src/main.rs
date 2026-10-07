//! 起動口: lane-priority.exe  （クライアント無しで画面を見るなら --mock、Data Dragon の更新確認を省くなら --offline）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Vec2};
use lane_core::analyzer::{Analysis, Analyzer, Overrides, UNCERTAIN_BELOW};
use lane_core::champions::{cache_path, refresh_cache, ChampionCatalog};
use lane_core::config::{Settings, BUNDLED_CHAMPIONS, BUNDLED_ROLES, BUNDLED_TRAITS};
use lane_core::matchup_db::MatchupDB;
use lane_core::models::{lane_label, position_label, tier_label, GameState, Placement, LANES, POSITIONS, TIERS};
use lane_core::scorer::LaneResult;
use lane_core::sources::{ClientSource, MockSource, Source};
use lane_core::traits::TraitTable;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

// ---- 色 ----
const BG: Color32 = Color32::from_rgb(0x14, 0x17, 0x1d);
const ROW_BG: Color32 = Color32::from_rgb(0x1b, 0x1f, 0x27);
const ROW_BORDER: Color32 = Color32::from_rgb(0x26, 0x2b, 0x35);
const DETAIL_BG: Color32 = Color32::from_rgb(0x16, 0x1a, 0x21);
const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe8, 0xeb);
const SUBTEXT: Color32 = Color32::from_rgb(0x9a, 0xa0, 0xa6);
const ALLY_COLOR: Color32 = Color32::from_rgb(0x4c, 0x8d, 0xff);
const ENEMY_COLOR: Color32 = Color32::from_rgb(0xff, 0x5a, 0x5f);
const EVEN_COLOR: Color32 = Color32::from_rgb(0x9a, 0xa0, 0xa6);
const UNKNOWN_COLOR: Color32 = Color32::from_rgb(0x5f, 0x66, 0x72);
const TRACK_COLOR: Color32 = Color32::from_rgb(0x2a, 0x2f, 0x3a);
const ADVICE_COLOR: Color32 = Color32::from_rgb(0xe0, 0xb8, 0x4c);

fn verdict_text(v: &str) -> &'static str {
    match v {
        "ally" => "味方有利",
        "even" => "互角",
        "enemy" => "敵有利",
        _ => "判定待ち",
    }
}

fn verdict_color(v: &str) -> Color32 {
    match v {
        "ally" => ALLY_COLOR,
        "even" => EVEN_COLOR,
        "enemy" => ENEMY_COLOR,
        _ => UNKNOWN_COLOR,
    }
}

fn arrows(score: f64) -> String {
    if score.abs() < 10.0 {
        return "―".into();
    }
    let n = if score.abs() < 30.0 { 1 } else if score.abs() < 60.0 { 2 } else { 3 };
    (if score > 0.0 { "◀" } else { "▶" }).repeat(n)
}

fn lane_positions(lane: &str) -> &'static [&'static str] {
    match lane {
        "bottom" => &["bottom", "utility"],
        "top" => &["top"],
        "jungle" => &["jungle"],
        _ => &["middle"],
    }
}

/// Windows の日本語フォントを読み込む（egui の既定フォントには日本語が無い）
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for path in [r"C:\Windows\Fonts\YuGothM.ttc", r"C:\Windows\Fonts\meiryo.ttc", r"C:\Windows\Fonts\msgothic.ttc"] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert("jp".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().insert(0, "jp".into());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}

// ---- 取得スレッドとのやりとり ----

enum Msg {
    State(GameState),
    Catalog(ChampionCatalog),
}

enum Cmd {
    SetLockfile(String),
    SetCatalog(ChampionCatalog),
}

/// 別スレッドで取得元を定期的に問い合わせる（クライアント探索で UI を止めないため）
fn spawn_poller(
    mut source: Box<dyn Source>,
    mut catalog: ChampionCatalog,
    interval: Duration,
    tx: Sender<Msg>,
    cmd_rx: Receiver<Cmd>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::SetLockfile(p) => source.set_lockfile(&p),
                Cmd::SetCatalog(c) => catalog = c,
            }
        }
        // 取得元の不具合でアプリごと落ちないようにする
        let state = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source.poll(&catalog)))
            .unwrap_or_else(|_| GameState { phase: "error".into(), status: "取得エラー".into(), ..Default::default() });
        if tx.send(Msg::State(state)).is_err() {
            return; // ウィンドウが閉じられた
        }
        ctx.request_repaint();
        std::thread::sleep(interval);
    });
}

// ---- 画面での操作（描画が終わってからまとめて反映する） ----

enum Action {
    Swap { side: &'static str, from: &'static str, to: &'static str },
    Reset,
    ToggleDetail(&'static str),
}

#[derive(Clone, Copy)]
struct DragSlot {
    side: &'static str,
    position: &'static str,
}

struct App {
    analyzer: Analyzer,
    settings: Settings,
    save_settings: bool,
    state: GameState,
    analysis: Analysis,
    overrides: Overrides,
    open_details: HashMap<&'static str, bool>,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    cmd_tx: Sender<Cmd>,
    settings_draft: Option<Settings>,
}

impl App {
    fn new(cc: &eframe::CreationContext, mock: bool, offline: bool) -> Self {
        setup_fonts(&cc.egui_ctx);
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = BG;
        visuals.window_fill = Color32::from_rgb(0x1b, 0x1f, 0x27);
        visuals.override_text_color = Some(TEXT);
        // Windows がライトテーマでも常にダーク表示にする
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        cc.egui_ctx.set_visuals_of(egui::Theme::Dark, visuals);
        cc.egui_ctx.style_mut(|s| {
            s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(14.0));
            s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(14.0));
        });

        let settings = Settings::load(None);
        let cache = cache_path();
        let catalog = ChampionCatalog::load_from_str(BUNDLED_CHAMPIONS, Some(&cache)).expect("同梱データが壊れています");
        let traits = TraitTable::from_json(BUNDLED_TRAITS, BUNDLED_ROLES).expect("同梱データが壊れています");
        let db = MatchupDB::open_if_exists(&settings.db_path);
        let (tx, rx) = mpsc::channel();
        let (cmd_tx, cmd_rx) = mpsc::channel();

        let source: Box<dyn Source> =
            if mock { Box::new(MockSource::default()) } else { Box::new(ClientSource::new(&settings.lockfile_path)) };
        spawn_poller(
            source,
            catalog.clone(),
            Duration::from_millis(settings.poll_interval_ms.max(100)),
            tx.clone(),
            cmd_rx,
            cc.egui_ctx.clone(),
        );
        if !offline {
            let tx = tx.clone();
            let ctx = cc.egui_ctx.clone();
            let current = catalog.version.clone();
            std::thread::spawn(move || {
                if let Some(new) = refresh_cache() {
                    if new.version != current {
                        let _ = tx.send(Msg::Catalog(new));
                        ctx.request_repaint();
                    }
                }
            });
        }

        let mut app = App {
            analyzer: Analyzer::new(catalog, traits, settings.clone(), db),
            settings,
            save_settings: true,
            state: GameState { status: "起動中…".into(), ..Default::default() },
            analysis: Analysis::default(),
            overrides: Overrides::new(),
            open_details: HashMap::new(),
            rx,
            tx,
            cmd_tx,
            settings_draft: None,
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        self.analyzer.settings = self.settings.clone();
        self.analysis = self.analyzer.analyze(&self.state, Some(&self.overrides));
    }

    fn save(&self) {
        if self.save_settings {
            let _ = self.settings.save(None);
        }
    }

    fn on_state(&mut self, state: GameState) {
        // 新しいチャンピオンセレクトが始まったら手動の入れ替えは捨てる
        if state.phase == "champ_select" && self.state.phase != "champ_select" {
            self.overrides.clear();
        }
        self.state = state;
        self.refresh();
    }

    /// side チームの from と to にいるチャンピオンを入れ替える（手動指定として記録）
    fn swap_positions(&mut self, side: &str, from: &str, to: &str) {
        if from == to {
            return;
        }
        let champ_at = |pos| self.analysis.placement_at(side, pos).and_then(|p| p.player.champion.clone());
        let (ca, cb) = (champ_at(from), champ_at(to));
        let ov = self.overrides.entry(side.to_string()).or_default();
        ov.retain(|c, p| p != from && p != to && Some(c) != ca.as_ref() && Some(c) != cb.as_ref());
        if let Some(ca) = ca {
            ov.insert(ca, to.to_string());
        }
        if let Some(cb) = cb {
            ov.insert(cb, from.to_string());
        }
        self.refresh();
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Swap { side, from, to } => self.swap_positions(side, from, to),
                Action::Reset => {
                    self.overrides.clear();
                    self.refresh();
                }
                Action::ToggleDetail(lane) => {
                    let e = self.open_details.entry(lane).or_default();
                    *e = !*e;
                }
            }
        }
    }

    // ---- 描画 ----

    fn top_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            match self.state.side.as_deref() {
                Some("blue") => {
                    ui.label(egui::RichText::new("味方: 青サイド").color(ALLY_COLOR).strong());
                }
                Some("red") => {
                    ui.label(egui::RichText::new("味方: 赤サイド").color(ENEMY_COLOR).strong());
                }
                _ => {}
            }
            let db_note = if self.analyzer.db.is_some() { "" } else { "　※実績DBなし（特性表のみで判定）" };
            ui.label(egui::RichText::new(format!("{}{db_note}", self.state.status)).color(SUBTEXT));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("設定").clicked() {
                    self.settings_draft = Some(self.settings.clone());
                }
                if ui.button("推定リセット").on_hover_text("入れ替えたポジションを元に戻します").clicked() {
                    actions.push(Action::Reset);
                }
                let before = self.settings.tier.clone();
                egui::ComboBox::from_id_salt("tier")
                    .selected_text(tier_label(&self.settings.tier).to_string())
                    .show_ui(ui, |ui| {
                        for t in std::iter::once("ALL").chain(TIERS) {
                            ui.selectable_value(&mut self.settings.tier, t.to_string(), tier_label(t));
                        }
                    });
                ui.label("ランク帯");
                if self.settings.tier != before {
                    self.save();
                    self.refresh();
                }
            });
        });
    }

    fn chip(&self, ui: &mut egui::Ui, side: &'static str, position: &'static str, multi: bool, width: f32, actions: &mut Vec<Action>) {
        let placement: Option<&Placement> = self.analysis.placement_at(side, position);
        let champion = placement.and_then(|p| p.player.champion.as_deref());
        let prefix = if multi { format!("{}  ", position_label(position)) } else { String::new() };

        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click_and_drag());
        let (fill, text_color, stroke, text, tooltip) = match (placement, champion) {
            (Some(p), Some(c)) => {
                let uncertain = !p.fixed && p.confidence < UNCERTAIN_BELOW;
                let mut text = prefix + self.analyzer.catalog.name(c);
                if uncertain {
                    text += " ?";
                }
                if p.player.hovering {
                    text += "（選択中）";
                }
                let (fill, fg, border) = if p.player.hovering {
                    (Color32::from_rgb(0x26, 0x2b, 0x35), Color32::from_rgb(0xc9, 0xcc, 0xd1), Color32::from_rgb(0x5f, 0x66, 0x72))
                } else if side == "ally" {
                    (Color32::from_rgb(0x1d, 0x31, 0x57), Color32::from_rgb(0xcf, 0xe0, 0xff), Color32::from_rgb(0x3a, 0x5f, 0xa8))
                } else {
                    (Color32::from_rgb(0x4a, 0x1f, 0x25), Color32::from_rgb(0xff, 0xd6, 0xd8), Color32::from_rgb(0xa8, 0x3a, 0x44))
                };
                let border = if uncertain { ADVICE_COLOR } else { border };
                let origin = if p.fixed {
                    "確定（クライアント情報 / 手動指定）".to_string()
                } else {
                    format!("推定（確信度 {:.0}%）", p.confidence * 100.0)
                };
                (fill, fg, border, text, Some(format!("{origin}\nドラッグで入れ替え / 右クリックでレーン変更")))
            }
            _ => (Color32::from_rgb(0x19, 0x1c, 0x22), UNKNOWN_COLOR, Color32::from_rgb(0x2f, 0x35, 0x40), prefix + "未選択", None),
        };

        // ドラッグ＆ドロップ: 同じチーム内でポジションを入れ替える
        if champion.is_some() {
            response.dnd_set_drag_payload(DragSlot { side, position });
        }
        let hovered_drop = response
            .dnd_hover_payload::<DragSlot>()
            .is_some_and(|s| s.side == side && s.position != position);
        if let Some(src) = response.dnd_release_payload::<DragSlot>() {
            if src.side == side && src.position != position {
                actions.push(Action::Swap { side, from: src.position, to: position });
            }
        }

        let painter = ui.painter();
        let stroke = Stroke::new(if hovered_drop { 2.0 } else { 1.0 }, if hovered_drop { TEXT } else { stroke });
        painter.rect(rect, CornerRadius::same(6), fill, stroke, StrokeKind::Inside);
        painter.text(rect.center(), Align2::CENTER_CENTER, &text, FontId::proportional(14.0), text_color);

        if response.dragged() {
            if let Some(pos) = ui.ctx().pointer_interact_pos() {
                let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("drag-ghost"));
                let ghost = Rect::from_center_size(pos, rect.size());
                let p = ui.ctx().layer_painter(layer);
                p.rect(ghost, CornerRadius::same(6), fill.gamma_multiply(0.85), Stroke::new(1.0, TEXT), StrokeKind::Inside);
                p.text(ghost.center(), Align2::CENTER_CENTER, &text, FontId::proportional(14.0), text_color);
            }
        }

        response.context_menu(|ui| {
            if champion.is_some() {
                for pos in POSITIONS {
                    if ui.add_enabled(pos != position, egui::Button::new(format!("{} にする", position_label(pos)))).clicked() {
                        actions.push(Action::Swap { side, from: position, to: pos });
                        ui.close();
                    }
                }
                ui.separator();
            }
            if ui.button("推定をリセット").clicked() {
                actions.push(Action::Reset);
                ui.close();
            }
        });
        if let Some(t) = tooltip {
            if !response.dragged() {
                response.on_hover_text(t);
            }
        }
    }

    fn score_bar(ui: &mut egui::Ui, result: &LaneResult, width: f32) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 46.0), Sense::click());
        let painter = ui.painter();
        let (margin, bar_h, y) = (8.0, 10.0, rect.top() + 6.0);
        let cx = rect.center().x;
        let half = rect.width() / 2.0 - margin;
        painter.rect_filled(
            Rect::from_min_max(egui::pos2(rect.left() + margin, y), egui::pos2(rect.right() - margin, y + bar_h)),
            CornerRadius::same(5),
            TRACK_COLOR,
        );
        let color = verdict_color(result.verdict);
        if let Some(score) = result.score {
            let len = half * (score.abs() / 100.0).min(1.0) as f32;
            let (x0, x1) = if score > 0.0 { (cx - len, cx) } else { (cx, cx + len) };
            if score != 0.0 {
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(x0, y), egui::pos2(x1, y + bar_h)),
                    CornerRadius::same(5),
                    color,
                );
            }
        }
        painter.rect_filled(
            Rect::from_min_max(egui::pos2(cx - 1.0, y - 3.0), egui::pos2(cx + 1.0, y + bar_h + 3.0)),
            CornerRadius::ZERO,
            Color32::from_rgb(0xd0, 0xd4, 0xda),
        );
        let text = match result.score {
            None => verdict_text("unknown").to_string(),
            Some(s) => format!("{}  {:+.0}  {}", arrows(s), s, verdict_text(result.verdict)),
        };
        painter.text(
            egui::pos2(cx, y + bar_h + 14.0),
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(15.0),
            color,
        );
        response
    }

    fn detail(&self, ui: &mut egui::Ui, result: &LaneResult) {
        egui::Frame::new().fill(DETAIL_BG).corner_radius(6).inner_margin(8).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let Some(score) = result.score else {
                let notes = if result.notes.is_empty() { "チャンピオンが揃うと判定します".to_string() } else { result.notes.join("\n") };
                ui.label(egui::RichText::new(notes).color(SUBTEXT));
                return;
            };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("判定の内訳").strong());
                ui.label(
                    egui::RichText::new(format!(
                        "実績 A {:.0}% / 特性 B {:.0}%（スコア {:+.1}）",
                        result.weight_a * 100.0,
                        result.weight_b * 100.0,
                        score
                    ))
                    .color(SUBTEXT),
                );
            });
            egui::Grid::new(("reasons", &result.lane)).spacing([14.0, 2.0]).show(ui, |ui| {
                for r in &result.reasons {
                    let color = if r.points > 0.05 { ALLY_COLOR } else if r.points < -0.05 { ENEMY_COLOR } else { EVEN_COLOR };
                    let tag = if r.source == "A" { "[実績]" } else { "[特性]" };
                    ui.label(egui::RichText::new(tag).color(SUBTEXT));
                    ui.label(&r.label);
                    ui.label(&r.detail);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(format!("{:+.1}", r.points)).color(color));
                    });
                    ui.end_row();
                }
            });
            let mut extra = vec![];
            if result.games > 0 {
                let tiers: Vec<&str> = result.tiers.iter().map(|t| tier_label(t)).collect();
                extra.push(format!(
                    "実績データ: {}（約{}試合）／ 選択ランク帯: {}",
                    tiers.join("+"),
                    result.games,
                    tier_label(&self.settings.tier)
                ));
            }
            extra.extend(result.notes.iter().cloned());
            for n in extra {
                ui.label(egui::RichText::new(format!("・{n}")).color(SUBTEXT));
            }
        });
    }

    fn lane_row(&self, ui: &mut egui::Ui, result: &LaneResult, actions: &mut Vec<Action>) {
        let lane: &'static str = LANES.iter().copied().find(|l| *l == result.lane).unwrap_or("top");
        let positions = lane_positions(lane);
        let multi = positions.len() > 1;
        egui::Frame::new()
            .fill(ROW_BG)
            .stroke(Stroke::new(1.0, ROW_BORDER))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let (ally_w, bar_w, enemy_w) = column_widths(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let label = ui.add_sized(
                        [48.0, 30.0],
                        egui::Label::new(egui::RichText::new(lane_label(lane)).size(17.0).strong()).sense(Sense::click()),
                    );
                    if label.clicked() {
                        actions.push(Action::ToggleDetail(lane));
                    }
                    for (side, w) in [("ally", ally_w), ("bar", bar_w), ("enemy", enemy_w)] {
                        if side == "bar" {
                            if Self::score_bar(ui, result, w).on_hover_text("クリックで根拠を表示").clicked() {
                                actions.push(Action::ToggleDetail(lane));
                            }
                            continue;
                        }
                        let side: &'static str = if side == "ally" { "ally" } else { "enemy" };
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            for pos in positions {
                                self.chip(ui, side, pos, multi, w, actions);
                            }
                        });
                    }
                });
                if self.open_details.get(lane).copied().unwrap_or(false) {
                    ui.add_space(6.0);
                    self.detail(ui, result);
                }
            });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let Some(draft) = self.settings_draft.as_mut() else { return };
        let mut done: Option<bool> = None;
        egui::Window::new("設定").collapsible(false).resizable(false).default_width(520.0).show(ctx, |ui| {
            egui::Grid::new("settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("lockfile の場所");
                ui.add(egui::TextEdit::singleline(&mut draft.lockfile_path).hint_text("空欄なら自動検出").desired_width(360.0));
                ui.end_row();
                ui.label("実績DB（SQLite）");
                ui.add(egui::TextEdit::singleline(&mut draft.db_path).desired_width(360.0));
                ui.end_row();
                ui.label("実績データの必要試合数")
                    .on_hover_text("この試合数に届かないマッチアップは、実績データの重みを下げて特性表に寄せます");
                ui.add(egui::DragValue::new(&mut draft.min_games).range(1..=100000));
                ui.end_row();
                ui.label("使う直近パッチ数");
                ui.add(
                    egui::DragValue::new(&mut draft.recent_patches)
                        .range(0..=50)
                        .custom_formatter(|v, _| if v == 0.0 { "全パッチ".into() } else { format!("{v:.0}") }),
                );
                ui.end_row();
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    done = Some(true);
                }
                if ui.button("キャンセル").clicked() {
                    done = Some(false);
                }
            });
        });
        match done {
            Some(true) => {
                let new = self.settings_draft.take().unwrap();
                if new.db_path != self.settings.db_path {
                    self.analyzer.db = MatchupDB::open_if_exists(&new.db_path);
                } else if let Some(db) = &self.analyzer.db {
                    db.clear_cache();
                }
                let _ = self.cmd_tx.send(Cmd::SetLockfile(new.lockfile_path.clone()));
                self.settings = new;
                self.save();
                self.refresh();
            }
            Some(false) => self.settings_draft = None,
            None => {}
        }
    }
}

/// 味方 : バー : 敵 = 2 : 3 : 2（レーン名 48px と間隔を除いた残り）
fn column_widths(total: f32) -> (f32, f32, f32) {
    let rest = (total - 48.0 - 12.0 * 3.0).max(300.0);
    (rest * 2.0 / 7.0, rest * 3.0 / 7.0, rest * 2.0 / 7.0)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::State(s) => self.on_state(s),
                Msg::Catalog(c) => {
                    let _ = self.cmd_tx.send(Cmd::SetCatalog(c.clone()));
                    self.analyzer.catalog = c;
                    self.refresh();
                }
            }
        }
        let _ = &self.tx;

        let mut actions = vec![];
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(14, 12)))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                self.top_bar(ui, &mut actions);

                // 見出し
                let (ally_w, bar_w, _) = column_widths(ui.available_width() - 26.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0 + 48.0 + 12.0);
                    ui.spacing_mut().item_spacing.x = 12.0;
                    ui.add_sized([ally_w, 16.0], egui::Label::new(egui::RichText::new("味方").color(SUBTEXT)));
                    ui.add_sized([bar_w, 16.0], egui::Label::new(egui::RichText::new("判定（左に伸びるほど味方有利）").color(SUBTEXT)));
                    ui.add_sized([ally_w, 16.0], egui::Label::new(egui::RichText::new("敵").color(SUBTEXT)));
                });

                egui::ScrollArea::vertical().show(ui, |ui| {
                    let lanes = self.analysis.lanes.clone();
                    for result in &lanes {
                        self.lane_row(ui, result, &mut actions);
                    }
                    if !self.analysis.advice.is_empty() {
                        egui::Frame::new().fill(ROW_BG).corner_radius(8).inner_margin(10).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let text: Vec<String> = self.analysis.advice.iter().map(|a| format!("▶ {a}")).collect();
                            ui.label(egui::RichText::new(text.join("　")).color(ADVICE_COLOR));
                        });
                    }
                    ui.label(
                        egui::RichText::new(
                            "レーン名・バーをクリックで根拠を表示 ／ 札をドラッグ（右クリック）でレーンを入れ替え ／ 「?」は推定の確信度が低いもの",
                        )
                        .color(SUBTEXT),
                    );
                });
            });
        self.settings_window(ctx);
        self.apply(actions);
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let mock = args.iter().any(|a| a == "--mock");
    let offline = args.iter().any(|a| a == "--offline");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 600.0]).with_min_inner_size([700.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native("レーン主導権", options, Box::new(move |cc| Ok(Box::new(App::new(cc, mock, offline)))))
}
