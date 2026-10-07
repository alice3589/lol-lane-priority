//! 起動口: lane-priority.exe  （クライアント無しで画面を見るなら --mock、Data Dragon の更新確認を省くなら --offline）
//!
//! 見た目は https://alice3589.github.io/MySite/ に合わせている:
//! 水色の背景にドット柄、ドット文字（DotGothic16）、黄色のアクセント、ぼかさない「ずらし影」、角の無い四角、
//! 「>」カーソル付きのボタン、黄色い四角＋点線の見出し。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shadow, Stroke, StrokeKind, Vec2};
use lane_core::analyzer::{Analysis, Analyzer, CounterPick, Overrides, UNCERTAIN_BELOW};
use lane_core::champions::{cache_path, refresh_cache, ChampionCatalog};
use lane_core::config::{Settings, BUNDLED_CHAMPIONS, BUNDLED_ROLES, BUNDLED_TRAITS};
use lane_core::matchup_db::MatchupDB;
use lane_core::models::{lane_label, position_label, tier_label, GameState, Placement, LANES, POSITIONS, TIERS};
use lane_core::scorer::LaneResult;
use lane_core::sources::{ClientSource, MockSource, Source};
use lane_core::traits::TraitTable;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

// ---- 色（MySite の style.css と同じ値） ----
const PAGE_BG: Color32 = Color32::from_rgb(0x51, 0xb9, 0xe3);
const ACCENT: Color32 = Color32::from_rgb(0xff, 0xe1, 0x5a);
const SHADOW: Color32 = Color32::from_rgb(0x2a, 0x7f, 0xa8);
const TEXT: Color32 = Color32::WHITE;
/// rgba(255,255,255,0.8)
const TEXT_SUB: Color32 = Color32::from_rgba_premultiplied(204, 204, 204, 204);
/// タグの文字色
const TAG_TEXT: Color32 = Color32::from_rgb(0x1f, 0x6d, 0x94);
/// 敵の色（サイトに無いので、水色の上で読める珊瑚色を足した）
const ENEMY: Color32 = Color32::from_rgb(0xff, 0x6b, 0x81);

/// カードの面。サイトは rgba(255,255,255,0.12) だが、半透明だと下のずらし影が透けて濃く見えるので、
/// 背景色に重ねた結果の色を不透明で使う
const CARD_BG: Color32 = Color32::from_rgb(0x66, 0xc1, 0xe6);

const FONT: f32 = 16.0;

fn white(alpha: f32) -> Color32 {
    Color32::from_white_alpha((alpha * 255.0) as u8)
}

fn hard_shadow(px: i8) -> Shadow {
    Shadow { offset: [px, px], blur: 0, spread: 0, color: SHADOW }
}

fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

fn verdict_text(v: &str) -> &'static str {
    match v {
        "ally" => "味方有利",
        "even" => "互角",
        "enemy" => "敵有利",
        _ => "判定待ち",
    }
}

/// 有利は黄色（アクセント）、不利は珊瑚色、互角は白
fn verdict_color(v: &str) -> Color32 {
    match v {
        "ally" => ACCENT,
        "enemy" => ENEMY,
        "even" => TEXT,
        _ => TEXT_SUB,
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

// ---- 描画の部品 ----

/// 文字の右下に影色の文字を重ねる（サイトの text-shadow: 2px 2px 0）
fn shadow_text(painter: &egui::Painter, pos: Pos2, anchor: Align2, text: &str, size: f32, color: Color32, offset: f32) -> Rect {
    painter.text(pos + Vec2::splat(offset), anchor, text, font(size), SHADOW);
    painter.text(pos, anchor, text, font(size), color)
}

/// 影付きの四角（角なし）
fn hard_rect(painter: &egui::Painter, rect: Rect, fill: Color32, offset: f32) {
    painter.rect_filled(rect.translate(Vec2::splat(offset)), CornerRadius::ZERO, SHADOW);
    painter.rect_filled(rect, CornerRadius::ZERO, fill);
}

/// 見出し: 黄色い四角 → 文字 → 残りの幅いっぱいの点線
fn section_heading(ui: &mut egui::Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
    let painter = ui.painter();
    let cy = rect.center().y;
    hard_rect(painter, Rect::from_center_size(Pos2::new(rect.left() + 6.0, cy), Vec2::splat(12.0)), ACCENT, 2.0);
    let r = shadow_text(painter, Pos2::new(rect.left() + 22.0, cy), Align2::LEFT_CENTER, text, 20.0, TEXT, 2.0);
    let mut x = r.right() + 12.0;
    while x + 3.0 <= rect.right() {
        painter.rect_filled(Rect::from_min_size(Pos2::new(x, cy - 1.5), Vec2::splat(3.0)), CornerRadius::ZERO, white(0.55));
        x += 7.0;
    }
}

/// 「>」カーソル付きのボタン。ホバー中はカーソルが右にずれて黄色になる
fn menu_link(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), font(FONT), TEXT);
    let size = Vec2::new(galley.size().x + 22.0, galley.size().y.max(22.0));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hovered = response.hovered();
    let color = if hovered { ACCENT } else { TEXT };
    let painter = ui.painter();
    let shift = if hovered { 4.0 } else { 0.0 };
    painter.text(Pos2::new(rect.left() + shift, rect.center().y), Align2::LEFT_CENTER, ">", font(FONT), ACCENT);
    shadow_text(painter, Pos2::new(rect.left() + 20.0, rect.center().y), Align2::LEFT_CENTER, text, FONT, color, 2.0);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// 背景: 水色に、上から薄く白いグラデーションと、16px 間隔の白いドット
fn paint_background(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, PAGE_BG);
    let mut mesh = egui::Mesh::default();
    let fade_y = rect.top() + rect.height() * 0.4;
    let top = white(0.14);
    let clear = Color32::TRANSPARENT;
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(Pos2::new(rect.right(), fade_y), clear);
    mesh.colored_vertex(Pos2::new(rect.left(), fade_y), clear);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
    let dot = white(0.18);
    let mut y = rect.top() + 8.0;
    while y < rect.bottom() {
        let mut x = rect.left() + 8.0;
        while x < rect.right() {
            painter.circle_filled(Pos2::new(x, y), 1.5, dot);
            x += 16.0;
        }
        y += 16.0;
    }
}

/// DotGothic16 を埋め込み、足りない記号（◀ ▶ など）は Windows の日本語フォントで補う
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "dot".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../assets/DotGothic16-Regular.ttf"))),
    );
    let mut order = vec!["dot".to_string()];
    for path in [r"C:\Windows\Fonts\YuGothM.ttc", r"C:\Windows\Fonts\meiryo.ttc", r"C:\Windows\Fonts\msgothic.ttc"] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert("jp".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            order.push("jp".into());
            break;
        }
    }
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        for (i, name) in order.iter().enumerate() {
            list.insert(i, name.clone());
        }
    }
    ctx.set_fonts(fonts);
}

/// ボタン・コンボボックス・設定ウィンドウなど、egui 標準の部品の見た目
fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PAGE_BG;
    v.window_fill = Color32::from_rgb(0x3f, 0xa6, 0xd0);
    v.window_stroke = Stroke::new(2.0, TEXT);
    v.window_shadow = hard_shadow(4);
    v.popup_shadow = hard_shadow(3);
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.override_text_color = Some(TEXT);
    v.extreme_bg_color = SHADOW; // 入力欄の背景
    v.selection.bg_fill = Color32::from_rgb(0x1f, 0x6d, 0x94);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    for (w, fill) in [
        (&mut v.widgets.noninteractive, Color32::TRANSPARENT),
        (&mut v.widgets.inactive, SHADOW),
        (&mut v.widgets.hovered, TAG_TEXT),
        (&mut v.widgets.active, TAG_TEXT),
        (&mut v.widgets.open, TAG_TEXT),
    ] {
        w.corner_radius = CornerRadius::ZERO;
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.fg_stroke = Stroke::new(1.0, TEXT);
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, white(0.55));
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        for style in [egui::TextStyle::Body, egui::TextStyle::Button, egui::TextStyle::Monospace] {
            s.text_styles.insert(style, font(FONT));
        }
        s.text_styles.insert(egui::TextStyle::Small, font(13.0));
        s.text_styles.insert(egui::TextStyle::Heading, font(20.0));
        s.spacing.button_padding = Vec2::new(8.0, 3.0);
        // クリックで根拠を開くときに、文字が選択されて青くならないようにする
        s.interaction.selectable_labels = false;
    });
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
    state: GameState,
    analysis: Analysis,
    overrides: Overrides,
    open_details: HashMap<&'static str, bool>,
    rx: Receiver<Msg>,
    cmd_tx: Sender<Cmd>,
    settings_draft: Option<Settings>,
    /// (ポジション, 敵チャンピオン) → カウンター候補。設定やチャンピオン一覧が変わったら捨てる
    counters: RefCell<HashMap<(&'static str, String), Vec<CounterPick>>>,
}

impl App {
    fn new(cc: &eframe::CreationContext, mock: bool, offline: bool) -> Self {
        setup_fonts(&cc.egui_ctx);
        setup_style(&cc.egui_ctx);

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
            state: GameState { status: "起動中…".into(), ..Default::default() },
            analysis: Analysis::default(),
            overrides: Overrides::new(),
            open_details: HashMap::new(),
            rx,
            cmd_tx,
            settings_draft: None,
            counters: RefCell::new(HashMap::new()),
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        self.analyzer.settings = self.settings.clone();
        self.analysis = self.analyzer.analyze(&self.state, Some(&self.overrides));
    }

    fn save(&self) {
        let _ = self.settings.save(None);
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

    /// 上部: アプリ名（ずらし影）と、黄色の状況表示（末尾の「_」が点滅）。右に操作
    fn header(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(250.0, 36.0), Sense::hover());
            shadow_text(ui.painter(), rect.left_center(), Align2::LEFT_CENTER, "lane priority", 32.0, TEXT, 3.0);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                if menu_link(ui, "設定").clicked() {
                    self.settings_draft = Some(self.settings.clone());
                }
                if menu_link(ui, "推定リセット").on_hover_text("入れ替えたポジションを元に戻します").clicked() {
                    actions.push(Action::Reset);
                }
                let before = self.settings.tier.clone();
                egui::ComboBox::from_id_salt("tier")
                    .selected_text(tier_label(&self.settings.tier).to_string())
                    .width(130.0)
                    .show_ui(ui, |ui| {
                        for t in std::iter::once("ALL").chain(TIERS) {
                            ui.selectable_value(&mut self.settings.tier, t.to_string(), tier_label(t));
                        }
                    });
                ui.label(egui::RichText::new("ランク帯").color(TEXT_SUB));
                if self.settings.tier != before {
                    self.save();
                    self.refresh();
                }
            });
        });

        // サイトの「programmer / speedrunner_」の行にあたるところ
        let side = match self.state.side.as_deref() {
            Some("blue") => "味方: 青サイド　/　",
            Some("red") => "味方: 赤サイド　/　",
            _ => "",
        };
        let blink = (ui.input(|i| i.time) * 2.0) as i64 % 2 == 0;
        ui.ctx().request_repaint_after(Duration::from_millis(500));
        let line = format!("{side}{}{}", self.state.status, if blink { "_" } else { "" });
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
        shadow_text(ui.painter(), rect.left_center(), Align2::LEFT_CENTER, &line, FONT, ACCENT, 2.0);
        if self.analyzer.db.is_none() {
            ui.label(egui::RichText::new("※実績DBなし（特性表のみで判定）").color(TEXT_SUB).size(13.0));
        }
    }

    fn chip(&self, ui: &mut egui::Ui, side: &'static str, position: &'static str, multi: bool, width: f32, actions: &mut Vec<Action>) {
        let placement: Option<&Placement> = self.analysis.placement_at(side, position);
        let champion = placement.and_then(|p| p.player.champion.as_deref());
        let prefix = if multi { format!("{}  ", position_label(position)) } else { String::new() };

        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click_and_drag());
        // 味方はサイトのタグ（白いカードに青い文字）、敵は珊瑚色のカード
        let (fill, text_color, border, shadow, text, tooltip) = match (placement, champion) {
            (Some(p), Some(c)) => {
                let uncertain = !p.fixed && p.confidence < UNCERTAIN_BELOW;
                let mut text = prefix + self.analyzer.catalog.name(c);
                if uncertain {
                    text += " ?";
                }
                if p.player.hovering {
                    text += "（選択中）";
                }
                let (fill, fg) = if p.player.hovering {
                    (white(0.2), TEXT)
                } else if side == "ally" {
                    (white(0.92), TAG_TEXT)
                } else {
                    (ENEMY, TEXT)
                };
                let border = if uncertain { Some(Stroke::new(2.0, ACCENT)) } else if p.player.hovering { Some(Stroke::new(1.0, TEXT)) } else { None };
                let origin = if p.fixed {
                    "確定（クライアント情報 / 手動指定）".to_string()
                } else {
                    format!("推定（確信度 {:.0}%）", p.confidence * 100.0)
                };
                (fill, fg, border, !p.player.hovering, text, Some(format!("{origin}\nドラッグで入れ替え / 右クリックでレーン変更")))
            }
            _ => (white(0.08), TEXT_SUB, Some(Stroke::new(1.0, white(0.45))), false, prefix + "未選択", None),
        };

        // ドラッグ＆ドロップ: 同じチーム内でポジションを入れ替える
        if champion.is_some() {
            response.dnd_set_drag_payload(DragSlot { side, position });
        }
        let drop_target = response
            .dnd_hover_payload::<DragSlot>()
            .is_some_and(|s| s.side == side && s.position != position);
        if let Some(src) = response.dnd_release_payload::<DragSlot>() {
            if src.side == side && src.position != position {
                actions.push(Action::Swap { side, from: src.position, to: position });
            }
        }

        let painter = ui.painter();
        if shadow {
            hard_rect(painter, rect, fill, 2.0);
        } else {
            painter.rect_filled(rect, CornerRadius::ZERO, fill);
        }
        let border = if drop_target { Some(Stroke::new(3.0, ACCENT)) } else { border };
        if let Some(b) = border {
            painter.rect_stroke(rect, CornerRadius::ZERO, b, StrokeKind::Inside);
        }
        painter.text(rect.center(), Align2::CENTER_CENTER, &text, font(FONT), text_color);

        if response.dragged() {
            if let Some(pos) = ui.ctx().pointer_interact_pos() {
                let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("drag-ghost"));
                let ghost = Rect::from_center_size(pos, rect.size());
                let p = ui.ctx().layer_painter(layer);
                hard_rect(&p, ghost, fill, 3.0);
                p.rect_stroke(ghost, CornerRadius::ZERO, Stroke::new(2.0, ACCENT), StrokeKind::Inside);
                p.text(ghost.center(), Align2::CENTER_CENTER, &text, font(FONT), text_color);
            }
        }

        response.context_menu(|ui| {
            if champion.is_some() {
                for pos in POSITIONS {
                    if ui.add_enabled(pos != position, egui::Button::new(format!("> {} にする", position_label(pos)))).clicked() {
                        actions.push(Action::Swap { side, from: position, to: pos });
                        ui.close();
                    }
                }
                ui.separator();
            }
            if ui.button("> 推定をリセット").clicked() {
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
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 48.0), Sense::click());
        let painter = ui.painter();
        let (margin, bar_h, y) = (8.0, 12.0, rect.top() + 6.0);
        let cx = rect.center().x.round();
        let half = rect.width() / 2.0 - margin;
        let track = Rect::from_min_max(Pos2::new(rect.left() + margin, y), Pos2::new(rect.right() - margin, y + bar_h));
        painter.rect_filled(track, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(0x2a, 0x7f, 0xa8, 150));
        let color = verdict_color(result.verdict);
        if let Some(score) = result.score {
            // 4px 刻みにしてドット絵っぽく見せる
            let len = ((half * (score.abs() / 100.0).min(1.0) as f32) / 4.0).round() * 4.0;
            if len > 0.0 {
                let (x0, x1) = if score > 0.0 { (cx - len, cx) } else { (cx, cx + len) };
                hard_rect(painter, Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, y + bar_h)), color, 2.0);
            }
        }
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(cx - 1.0, y - 4.0), Pos2::new(cx + 1.0, y + bar_h + 4.0)),
            CornerRadius::ZERO,
            TEXT,
        );
        let text = match result.score {
            None => verdict_text("unknown").to_string(),
            Some(s) => format!("{}  {:+.0}  {}", arrows(s), s, verdict_text(result.verdict)),
        };
        shadow_text(painter, Pos2::new(cx, y + bar_h + 15.0), Align2::CENTER_CENTER, &text, FONT, color, 2.0);
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        response
    }

    /// 敵のチャンピオンごとに「それに有利なチャンピオン」の上位を並べる
    fn counter_section(&self, ui: &mut egui::Ui, result: &LaneResult) {
        let positions = lane_positions(&result.lane);
        let picked: Vec<&str> = self
            .analysis
            .ally
            .iter()
            .chain(&self.analysis.enemy)
            .filter_map(|p| p.player.champion.as_deref())
            .collect();
        for pos in positions {
            let Some(enemy) = self.analysis.placement_at("enemy", pos).and_then(|p| p.player.champion.clone()) else { continue };
            let mut cache = self.counters.borrow_mut();
            let list = cache
                .entry((pos, enemy.clone()))
                .or_insert_with(|| self.analyzer.counter_picks(pos, &enemy, 30));
            let shown: Vec<&CounterPick> = list.iter().filter(|c| !picked.contains(&c.champion.as_str())).take(6).collect();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("カウンター候補").color(ACCENT));
                let who = format!("{}（{}）", self.analyzer.catalog.name(&enemy), position_label(pos));
                ui.label(egui::RichText::new(format!("{who} に有利（点数が高いほど有利）")).color(TEXT_SUB));
            });
            ui.horizontal_wrapped(|ui| {
                for c in shown {
                    let text = format!("{} {:+.0}", self.analyzer.catalog.name(&c.champion), c.score);
                    let r = ui.label(egui::RichText::new(text).color(TAG_TEXT).background_color(white(0.92)));
                    if !c.reason.is_empty() {
                        r.on_hover_text(format!("主な理由: {}", c.reason));
                    }
                }
            });
            ui.add_space(4.0);
        }
    }

    fn detail(&self, ui: &mut egui::Ui, result: &LaneResult) {
        egui::Frame::new()
            .fill(Color32::from_rgba_unmultiplied(0x1f, 0x6d, 0x94, 110))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.counter_section(ui, result);
                let Some(score) = result.score else {
                    let notes = if result.notes.is_empty() { "チャンピオンが揃うと判定します".to_string() } else { result.notes.join("\n") };
                    ui.label(egui::RichText::new(notes).color(TEXT_SUB));
                    return;
                };
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("判定の内訳").color(ACCENT));
                    ui.label(
                        egui::RichText::new(format!(
                            "実績 A {:.0}% / 特性 B {:.0}%（スコア {:+.1}）",
                            result.weight_a * 100.0,
                            result.weight_b * 100.0,
                            score
                        ))
                        .color(TEXT_SUB),
                    );
                });
                egui::Grid::new(("reasons", &result.lane)).spacing([18.0, 3.0]).show(ui, |ui| {
                    for r in &result.reasons {
                        let color = if r.points > 0.05 { ACCENT } else if r.points < -0.05 { ENEMY } else { TEXT_SUB };
                        let tag = if r.source == "A" { "実績" } else { "特性" };
                        ui.label(egui::RichText::new(tag).color(TAG_TEXT).background_color(white(0.92)));
                        ui.label(&r.label);
                        ui.label(egui::RichText::new(&r.detail).color(TEXT_SUB));
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
                    ui.label(egui::RichText::new(format!("・{n}")).color(TEXT_SUB));
                }
            });
    }

    /// 1レーン分のカード（サイトの speedrun の行と同じ: 白の薄い面、左に黄色の帯、ずらし影）
    fn lane_row(&self, ui: &mut egui::Ui, result: &LaneResult, actions: &mut Vec<Action>) {
        let lane: &'static str = LANES.iter().copied().find(|l| *l == result.lane).unwrap_or("top");
        let positions = lane_positions(lane);
        let multi = positions.len() > 1;
        let frame = egui::Frame::new()
            .fill(CARD_BG)
            .shadow(hard_shadow(3))
            .inner_margin(egui::Margin { left: 16, right: 12, top: 8, bottom: 8 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let (ally_w, bar_w, enemy_w) = column_widths(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let (rect, label) = ui.allocate_exact_size(Vec2::new(LABEL_W, 30.0), Sense::click());
                    let color = if label.hovered() { ACCENT } else { TEXT };
                    shadow_text(ui.painter(), rect.left_center(), Align2::LEFT_CENTER, lane_label(lane), 24.0, color, 2.0);
                    if label.on_hover_text("クリックで根拠を表示").clicked() {
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
                            ui.spacing_mut().item_spacing.y = 6.0;
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
        let r = frame.response.rect;
        ui.painter().rect_filled(Rect::from_min_size(r.min, Vec2::new(4.0, r.height())), CornerRadius::ZERO, ACCENT);
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let Some(draft) = self.settings_draft.as_mut() else { return };
        let mut done: Option<bool> = None;
        egui::Window::new(egui::RichText::new("設定").color(TEXT)).collapsible(false).resizable(false).default_width(560.0).show(ctx, |ui| {
            egui::Grid::new("settings").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
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
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 20.0;
                if menu_link(ui, "OK").clicked() {
                    done = Some(true);
                }
                if menu_link(ui, "キャンセル").clicked() {
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
                self.counters.borrow_mut().clear();
                self.save();
                self.refresh();
            }
            Some(false) => self.settings_draft = None,
            None => {}
        }
    }
}

const LABEL_W: f32 = 52.0;

/// 味方 : バー : 敵 = 2 : 3 : 2（レーン名と間隔を除いた残り）
fn column_widths(total: f32) -> (f32, f32, f32) {
    let rest = (total - LABEL_W - 12.0 * 3.0).max(300.0);
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
                    self.counters.borrow_mut().clear();
                    self.refresh();
                }
            }
        }

        let mut actions = vec![];
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(PAGE_BG).inner_margin(egui::Margin { left: 28, right: 28, top: 20, bottom: 12 }))
            .show(ctx, |ui| {
                paint_background(ui);
                ui.spacing_mut().item_spacing.y = 8.0;
                self.header(ui, &mut actions);
                ui.add_space(10.0);

                egui::ScrollArea::vertical().show(ui, |ui| {
                    // 右端のずらし影がスクロールバーに隠れないよう、少し内側に寄せる
                    ui.set_max_width(ui.available_width() - 8.0);
                    section_heading(ui, "lanes");
                    let (ally_w, bar_w, enemy_w) = column_widths(ui.available_width() - 28.0);
                    ui.horizontal(|ui| {
                        ui.add_space(16.0 + LABEL_W + 12.0);
                        ui.spacing_mut().item_spacing.x = 12.0;
                        for (text, w) in [("味方", ally_w), ("判定（左に伸びるほど味方有利）", bar_w), ("敵", enemy_w)] {
                            ui.add_sized([w, 16.0], egui::Label::new(egui::RichText::new(text).color(TEXT_SUB).size(13.0)));
                        }
                    });
                    ui.spacing_mut().item_spacing.y = 12.0;
                    let lanes = self.analysis.lanes.clone();
                    for result in &lanes {
                        self.lane_row(ui, result, &mut actions);
                    }

                    if !self.analysis.advice.is_empty() {
                        ui.add_space(10.0);
                        section_heading(ui, "advice");
                        ui.spacing_mut().item_spacing.y = 4.0;
                        for a in &self.analysis.advice {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(">").color(ACCENT));
                                ui.label(a);
                            });
                        }
                    }
                    ui.add_space(14.0);
                    ui.label(
                        egui::RichText::new(
                            "レーン名・バーをクリックで根拠とカウンター候補を表示 ／ 札をドラッグ（右クリック）でレーンを入れ替え ／ 「?」は推定の確信度が低いもの",
                        )
                        .color(TEXT_SUB)
                        .size(13.0),
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
        viewport: egui::ViewportBuilder::default().with_inner_size([920.0, 680.0]).with_min_inner_size([720.0, 460.0]),
        ..Default::default()
    };
    eframe::run_native("lane priority", options, Box::new(move |cc| Ok(Box::new(App::new(cc, mock, offline)))))
}
